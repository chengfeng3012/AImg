# AndroidImg_Rust (`aimg`) — 对抗性缺陷审计报告

审计范围：`src/`（util / sparse / superimg / boot / payload / dat / perm / ext4::disk|read|write / main）
对照上游：`vendor/`（liblp、libsparse、mkbootimg、update_engine proto、payload_dumper(-go)、sdat2img/img2sdat、imgpatchtools、ext4_utils）
验证方式：静态分析为主；关键结论在 `ssh aws`（Ubuntu 24.04，e2fsprogs 1.47.0）上用 `/root/AndroidImg_Final/target/release/aimg` + `mkfs.ext2/4` + `e2fsck -fn` + `debugfs` 复现，脚本见 `audit/probes/aimgv1..6.sh`。
未修改 `src/`、`tests/`、`Cargo.toml`、`configs/`，远端只写 `/tmp`。

严重度统计：**critical 2 / high 5 / medium 6 / low 7**

---

## Critical

### C1. 多块组 ext4（>128 MiB @4K）的组描述符位图与 backup superblock/GDT 重叠，产物不可用

**位置**：`src/ext4/write.rs:174-204`（布局）、`src/ext4/write.rs:498-525`（backup 写入）

**问题**
组 0 的布局是对的（`g0_bb = 1 + gdt_blocks`，即 block 0 = boot/SB、block 1.. = 主 GDT、其后才是位图）；
但 `for g in 1..groups` 分支把**每个组的前两块**直接当 block bitmap / inode bitmap：

```rust
let start = first_data_block as u64 + g * bpg as u64;
let bb = start;          // ← 该块在有 backup superblock 的组里是 backup SB
let ib = start + 1;      // ← 该块是 backup GDT
let it = start + 2;
```

而 `sparse_super_group(g)` 对 g=1 以及 3/5/7 的幂次组都为真，之后又把这些 backup 写回同一批块：

```rust
write_at(&mut f, start * bs as u64 + 1024, &sb_bytes)?;      // 落在 block `start`
write_at(&mut f, start * bs as u64 + bs, &gdt_bytes)?;       // 落在 block `start+1`
```

于是：block bitmap 被 superblock 内容覆盖一半、inode bitmap 被组描述符覆盖；同时 `start..start+1+gdt_blocks`
这些 backup 块从未 `mark_block`，被算成空闲。ext4 规范里带 backup superblock 的组，头部顺序是
`[backup SB][backup GDT ...][block bitmap][inode bitmap][inode table]`，两个位图在 SB+GDT 之后。
上游可直接对照：`vendor/lpunpack_and_lpmake/lib/ext4_utils/ext4_utils.cpp:126-131`

```c
u32 last_header_size = 2 + aux_info.inode_table_blocks;          /* 2 = block+inode bitmap */
if (ext4_bg_has_super_block((int)aux_info.groups - 1))
        last_header_size += 1 + aux_info.bg_desc_blocks + ...   /* SB + backup GDT 在位图之前 */
```

**复现**（`audit/probes/aimgv.sh`，256 MiB → 2 组）

```
aimg ext4 build --dir tree --meta meta.txt --out s256.img --size 256M
e2fsck -fn s256.img
```

```
ext2fs_check_desc: Corrupt group descriptor: bad block for block bitmap
e2fsck: Group descriptors look bad... trying backup blocks...
Group 1's block bitmap at 32768 conflicts with some other fs block.
Relocate? no
Group 1's inode bitmap at 32769 conflicts with some other fs block.
Block bitmap differences:  -40974 -41008 ... (大量)
Free blocks count wrong for group #1 (32254, counted=32064).
Inode bitmap differences:  -8194 -(8225--8226) ...
Free inodes count wrong for group #1 (8192, counted=8146).
system: ********** WARNING: Filesystem still has errors **********
```

`dumpe2fs` 自证重叠：

```
Group 1: (Blocks 32768-65535)
  Backup superblock at 32768, Group descriptors at 32769-32769
  Block bitmap at 32768 (+0)
  Inode bitmap at 32769 (+1)
  Inode table at 32770-33281 (+2)
```

对照：32 MiB（单组）同一棵树的 `e2fsck -fn` 是干净的（exit 0），所以本项目 README §8 的 64M/32M 验证永远发现不了这个问题。
只要镜像 ≥ 2 组（4K 块：>128 MiB；1K 块：>8 MiB）即损坏，而真实 system.img 基本都是多组。

**影响**：产物不可挂载/不可写（位图把大量在用块标成空闲，挂载后写会覆盖数据）；即使只读挂载也可能被内核判为
`bad block for block bitmap`。

**修复**

```rust
for g in 1..groups {
    let start = first_data_block as u64 + g as u64 * bpg as u64;
    let hdr = if sparse_super_group(g as u64) { 1 + gdt_blocks as u64 } else { 0 };
    for b in start..start + hdr { mark_block(&mut used, b)?; }   // backup SB + GDT 必须占用
    let bb = start + hdr;
    let ib = bb + 1;
    let it = ib + 1;
    ...
}
```

backup 写入处也要跟着改（`off = start + hdr - gdt_blocks` 起写 GDT，SB 在 `start`），并且
`gdt_bytes` 只覆盖 `gdt_blocks` 块。

---

### C2. `--block-size 1024` 时组 0 布局整体错位一块（block bitmap 落在 GDT 块上）

**位置**：`src/ext4/write.rs:156`（`first_data_block`）、`174`（`g0_bb`）、`514`（`gdt_off`）、`506`（backup SB）

**问题**
`let g0_bb = 1 + gdt_blocks;` 只有在 `first_data_block == 0`（bs ≥ 2048）时成立。
`bs == 1024` 时 `first_data_block = 1`：block 0 = boot、block 1 = superblock（`write_at(f, 1024, sb)`）、
block 2.. = 主 GDT（`gdt_off = (first_data_block+1)*bs`）。而 `g0_bb = 2`，正好是 GDT 块：
位图先写，随后 `write_at(f, gdt_off, &gdt_bytes)` 把它们覆盖掉，组 0 的 block bitmap 内容变成组描述符。
同类 off-by-one 还出现在 backup superblock：`write_at(f, start*bs + 1024, sb)` 对 1K 块应写 `start*bs`。

**复现**（`aimgv.sh`，32 MiB + `--block-size 1024`）

```
ext2fs_check_desc: Corrupt group descriptor: bad block for block bitmap
Group 0's block bitmap at 2 conflicts with some other fs block.
Group 1's block bitmap at 8193 conflicts with some other fs block.
Group 1's inode bitmap at 8194 conflicts with some other fs block.
Block bitmap differences:  +1 +(3--32) ...
Free blocks count wrong for group #0 (7469, counted=8112).
```

**修复**

```rust
let g0_bb = first_data_block as u64 + 1 + gdt_blocks as u64;
```
`mark_block` 的范围改为 `0..gdt_blocks` 加上 `first_data_block`；backup SB 偏移按
`if bs == 1024 { start * bs } else { start * bs + 1024 }` 取。

---

## High

### H1. `super unpack` 把 `_a`/`_b` 剥成同名 → 静默互相覆盖，默认参数得到错槽镜像

**位置**：`src/superimg.rs:430-441`

**问题**
真实 super.img 的 metadata **同时**列出 `system_a` 与 `system_b`（lpunpack 因此会导出两份）。
本实现把后缀剥掉：

```rust
if let Some(stripped) = name.strip_suffix("_a") {
    if opt.slot.unwrap_or(0) == 1 { name = format!("{}_b", stripped); }   // 忽略 keep_suffix
    else if !opt.keep_suffix { name = stripped.to_string(); }
} else if let Some(stripped) = name.strip_suffix("_b") {
    if opt.slot.unwrap_or(0) == 0 && !opt.keep_suffix { name = stripped.to_string(); }
}
```

`--slot 0` + 默认 `keep_suffix=false`：`system_a`→`system.img`、`system_b`→`system.img`；
`create_sized` 带 `truncate(true)`，后写的 `system_b` 直接覆盖 `system_a`。
`--slot 1` 时 `system_a`→`system_b.img`，与真正的 `system_b` 再次同名覆盖。
`keep_suffix` 只在其中一条分支生效，`--slot 1 --keep-suffix` 依然重命名。
上游 lpunpack 完全不做后缀改写（`vendor/lpunpack_and_lpmake/partition_tools/lpunpack.cc:233`
`output_path = output_dir_ + "/" + GetPartitionName(*partition) + ".img"`）。

**复现**（`aimgv2.sh`，用 `super make` 造 `system_a`=100000B、`system_b`=120000B）

```
$ aimg super unpack super.img --out out0 --slot 0
out0/system.img          (打印两次)
$ stat -c %s out0/system.img   → 122880
$ cmp <(head -c 120000 out0/system.img) b.img → MATCH_B     # 得到的是 system_b
$ aimg super unpack super.img --out out1 --slot 1
out1/system_b.img        (打印两次)
```

**影响**：`--slot 0` 却拿到 `_b` 的内容（README §4 的标准流程就会踩），且无任何警告；分区间互相覆盖无法察觉。

**修复**：全程保留原分区名（与 lpunpack 一致）；若确实要剥离，先构建 `name -> 源分区` 映射，
发现重名即 `bail!`（或自动回退到带后缀），并按 `slot` 只导出与该槽位吻合的后缀。

---

### H2. vendor_boot v4 重新打包时忽略分片内容/长度：表项与 ramdisk 段不一致

**位置**：`src/boot.rs:563-580`（`pack`）、`src/boot.rs:431-444`（`build_vendor` 表项）、`src/boot.rs:447-453`（段布局）

**问题**
`unpack` 同时产出 `vendor_ramdisk`（整段原始字节）和 `vendor_ramdisk.<name>`（每个分片）。
`pack` 里：

```rust
img.ramdisk = read_opt("vendor_ramdisk")?;     // 整段仍来自原始文件
for rd in &desc.vendor_ramdisks {              // 分片文件只用来重算 offset/size
    let data = read_opt(&name)?;
    d.offset = offset; d.size = data.len() as u32; offset += data.len() as u32;
```

`build_vendor` 写头时用 `self.ramdisk.len()` 作为 `vendor_ramdisk_size`，写表项时用分片文件的 `data.len()`/累计 `offset`。
只要用户编辑了任一分片（长度变化），表里的 `[offset, offset+size)` 就与真实段内容错位：超界的分片读成空，
未超界的读到旧数据的错位片段。

**复现**（`aimgv2.sh`，v4，platform=100B + dlkm=50B，整段 150B；把 platform 改成 110B 后重打包）

```
aimg boot pack vb/ --out vb2.img          # 成功，无警告
aimg boot unpack vb2.img --out vb_check
-rw-r--r-- vendor_ramdisk          150      # 仍是旧段
-rw-r--r-- vendor_ramdisk.dlkm       0      # 表项 [110,160) 越界 → 空
-rw-r--r-- vendor_ramdisk.platform 110      # 内容 = 旧段前 110 字节（platform 100B + dlkm 前 10B）
```

**影响**：设备侧 `vendor_ramdisk_table` 指向错误数据 → vendor_boot 无法启动（first-stage init 找不到 DLKM/平台分片），
且工具全程无报错。

**修复**：`build_vendor` 里由分片重新拼接整段再写：

```rust
let mut raw = Vec::new();
for (rd, data) in &self.vendor_ramdisks { rd.offset = raw.len() as u32; raw.extend_from_slice(data); }
// hdr[24..28] = raw.len(), 段内容写 raw
```
或在长度与原始表项不符时直接 `bail!`。

---

### H3. `payload make --compress` 静默不压缩；一旦真的压缩，`data_sha256` 又是错的

**位置**：`src/payload.rs:682-695`（压缩分支）、`src/payload.rs:675`（`data_sha256`）

**问题 1（已实测）**：

```rust
lzma_rs::xz_compress(&mut std::io::Cursor::new(chunk), &mut enc).ok();   // 错误被丢弃
if !enc.is_empty() && enc.len() < chunk.len() { ... OP_REPLACE_XZ ... }
```

`xz_compress` 的 `Err` 被 `.ok()` 吞掉，`enc` 为空 → 永远退回 `OP_REPLACE`。用高度可压缩输入
（`"Android payload test line\n"*12000`，312000B）实测：`--compress` 后唯一 op 仍是
`data_length == 312000`、`data_sha256 == sha256(原图块)`，即**一个字节都没压**且用户不知情。

**问题 2（潜在，一旦压缩生效）**：`data_sha256` 取的是**压缩前**分块的哈希：

```rust
let mut op = InstallOp { ..., data_sha256: Sha256::digest(chunk).to_vec(), ... };
```

而 `data_sha256_hash` 的语义是“存储 blob 的哈希”。两份参考实现都在**解压前**校验原始 blob：
`vendor/payload_dumper/payload_dumper.py:130-134`

```python
data = payload_file.read(op.data_length)
if op.data_sha256_hash:
    assert hashlib.sha256(data).digest() == op.data_sha256_hash
if op.type == op.REPLACE_XZ: data = dec.decompress(data)
```
`vendor/payload-dumper-go/payload/payload.go:64,70`（`verifyBlobHash(op, sha256.Sum256(buf))`）同样如此。
所以带 `--compress` 的包在 payload-dumper / payload_dumper / 设备端 `DeltaPerformer` 上会报
`operation data hash mismatch`。

**修复**

```rust
let stored = match compress_chunk(chunk)? {           // ? 传出去，不要 .ok()
    Some(comp) => { op.ty = OP_REPLACE_XZ; comp }
    None => chunk.to_vec(),
};
op.data_length = stored.len() as u64;
op.data_sha256 = Sha256::digest(&stored).to_vec();    // 对“存储形态”取哈希
```

---

### H4. ext4 读取非 extent 文件（ext2/ext3、无 extents 特性）只读 12 个直接块，其余静默补零

**位置**：`src/ext4/read.rs:81-92`（`extents()` 的 classic 分支）、`src/ext4/read.rs:157`（`out.resize(size, 0)`）

**问题**：非 extents 文件只遍历 `inode.block[..12]` 直接块，单/双/三级间接块完全没实现；
`read_data` 末尾 `out.resize(size, 0)` 把缺失部分补齐成 0，于是**长度对、内容错**，没有任何提示。

**复现**（`aimgv3.sh`：`mkfs.ext2` 造 32 MiB、含 300000B 文件的镜像）

```
aimg ext4 extract e2.img --out o2
cmp tree/big.bin o2/big.bin
→ differ: byte 49153        # = 12*4096+1，正好第 13 个块
stat -c %s o2/big.bin → 300000   # 长度正确，所以肉眼很难发现
```

**影响**：解包 ext2/ext3 或 `-O ^extent` 的镜像得到“看起来正常”的坏文件（APK/ELF 尾部为 0）。

**修复**：实现 `i_block[12..15]` 的间接块遍历（block_size/4 项/块，注意 `blocks_count` 边界），
并把不支持的形态改成 `bail!`（宁可失败也不要静默补零）：

```rust
if !inode.has_extents() && inode.tree_depth_unsupported() { bail!("indirect blocks not supported for inode {}", ino) }
```

---

### H5. `.transfer.list` 的 `bsdiff`/`move`/`stash` 语法与真实格式不符 → 差分 OTA 完全无法处理

**位置**：`src/dat.rs:40-56`（`rangeset`）、`src/dat.rs:100-116`（`parse_transfer_list`）、`src/dat.rs:257-266`（`patch` 的 `Cmd::Bsdiff`）

**问题**
真实 transfer.list 的差分命令把补丁位置/长度放在**行内**，补丁文件本身是一整段拼接数据：

```text
bsdiff <patch_offset> <patch_len> <src_rangeset> <dst_rangeset>                  (v1/v2)
bsdiff <patch_offset> <patch_len> <src_hash> <dst_hash> <src_rangeset> <dst_rangeset>   (v3/v4)
move   <src_rangeset> <dst_rangeset>
stash  <stash_id> <rangeset>      free <stash_id>
```

出处：`vendor/imgpatchtools/blockimg/blockimg.cpp:1690-1700`（命令语法注释）、
`1250-1268`（`PerformCommandDiff` 先读 `<offset> <length>` 两个 token，再 `patch_start + offset` 取数据）、
`1021-1032`（v3/v4 先吃掉 1~2 个 hash token 再读 rangeset）。

本实现把 `bsdiff` 行解析成 `<src_rangeset> <dst_rangeset>` 两个字段，并且用自造的
`u32 LE 长度 + patch` 分帧（`dat.rs:258-262`）：

```rust
"bsdiff" => { let src = rangeset(it.next()?)?; let dst = rangeset(it.next()?)?; ... }
```
对真实 TL：`it.next()` 拿到的是 `patch_offset`（如 `"1234"`），`rangeset("1234")` 立刻
`bail!("rangeset count mismatch")`。即使补丁位置碰巧是合法 rangeset 文本，v3/v4 的两个 hash
token 也会让 src/dst 错位。同时 `move` / `stash` / `free` 不在 match 分支里，
`parse_transfer_list` 直接 `bail!("unknown transfer list command 'move'")`，
所以连 `aimg dat info` 都读不了真实差分 TL。

README §7 声称“`.dat` 的 bsdiff 补丁流约定为 u32 LE 长度 + 数据分帧（与 imgpatchtools 的
BlockImageUpdate 一致）”——与上游实现不符：blockimg 的补丁数据是 mmap 的整段数据，
每段的位置/长度来自命令行 token，流里没有长度前缀。

**复现**：取任意差分 OTA 的 `system.transfer.list`（含 `bsdiff 0 512 2,0,8 ...` 或 `move ...` 行）：
`aimg dat info <tl>` → `error: rangeset count mismatch in '0'` / `error: unknown transfer list command 'move'`。
项目自带用例（`dat.rs:449` 的 `bsdiff 2,0,8 2,0,8` + `u32 len` 分帧）只能验证自造格式，不覆盖真实格式。

**修复**

1. `Cmd::Bsdiff { patch_off: u64, patch_len: u64, src, dst }`、新增 `Cmd::Move { src, dst }`、
   `Cmd::Stash { id: u32, src }`、`Cmd::Free { id: u32 }`；
2. `parse_transfer_list`：`bsdiff` 先解析两个十进制数作 patch 位置/长度，v>=3 时再吞掉 1~2 个
   十六进制 hash token，最后才是两个 rangeset；`move` 两个 rangeset；`stash`/`free` 按 id；
3. `patch()`：改为 `pd.seek(patch_off); read_exact(patch_len)`，删掉 u32 前缀假设；
   `move` 直接等同 `Cmd::Copy`。

---

## Medium

### M1. boot v2/v4 头部 `header_size` 字段写错（1648/1580，规范是 1660/1584）

**位置**：`src/boot.rs:383`（v1/v2 都写 1648）、`src/boot.rs:358`（v3/v4 都写 1580）

上游常量（`vendor/aosp_mkbootimg/mkbootimg.py:40-44`）：

```python
BOOT_IMAGE_HEADER_V1_SIZE = 1648
BOOT_IMAGE_HEADER_V2_SIZE = 1660
BOOT_IMAGE_HEADER_V3_SIZE = 1580
BOOT_IMAGE_HEADER_V4_SIZE = 1584
```
写入点是 `mkbootimg.py:127`（v4 写 1584）、`256-259`（v2 写 1660）。
本实现 v4 同时把 `signature_size` 写在 1580..1584，却声明头长 1580 —— 声明范围装不下自己的字段；
v2 少报 12 字节（dtb_size/dtb_addr 落在声明之外）。

**复现**（`aimgv2.sh`，`boot pack` 后用 `od` 读字段）

```
header_size@20 (expect 1584):   1580
v2 header_size@1644 (expect 1660): 1648
```

**影响**：Android 侧 `unpack_bootimg` 只读不校验，但声明范围自相矛盾的头部会让按 `header_size`
定位后续数据的工具/引导链读到偏移错误的段；v2 少 12 字节同理。

**修复**：按版本写 `1580 + if v>=4 {4} else {0}`、`1648 + if v==2 {12} else {0}`。

---

### M2. `ext4 build`：meta 里缺失的中间目录被“拍平”到根

**位置**：`src/ext4/write.rs:92-118`

**问题**：补建缺失父目录时，先建最深的 `cur`，其父 `pp` 可能还不存在，于是 `unwrap_or(&0)` 把它挂到根：

```rust
let parent_idx = *index.get(&pp).unwrap_or(&0);   // pp 还没建 → 挂到 root
nodes.push(Node { path: cur, parent: Some(parent_idx), ... });
```
下一轮再建 `pp` 时，已建好的 `cur` 不会被重新挂到 `pp` 下（`children` 已经写进 root）。

**复现**（`aimgv5.sh`：meta.txt 里只有 `F /a/b/c.txt`，树里也只有一个文件）

```
aimg ext4 build --dir tree --meta meta.txt --out deep.img --size 32M
aimg ext4 ls deep.img      →  a/ \n b/ \n   c.txt        # b 在根，不在 a 下
debugfs -R "ls -l /a"      →  只有 . 和 ..               # /a 是空的，/a/b 根本不存在
```
实际结构是 `root{ a, b{ c.txt } }`，而不是 `root{ a{ b{ c.txt } } }`；e2fsck 干净（结构自洽），
所以只能靠内容核对发现。

**影响**：只要 meta.txt 是手工编辑/裁剪过的（README 明确鼓励手改），目录层级会静默变形。

**修复**：递归保证祖先存在（先 `ensure(parent)` 再挂载），例如自顶向下遍历 `path.split('/')`，
或把 `unwrap_or(&0)` 改成先递归创建 `pp` 再取索引。

---

### M3. 镜像尺寸刚过组边界时构建直接失败（`layout overflow`）

**位置**：`src/ext4/write.rs:158`（`groups`）、`184-186` / `198-200`（每组都按满 `itable_blocks` 占位）、`206`

**问题**：每组无条件保留 `ipg * inode_size` 的 inode table（8192×256/4096 = 512 块），
最后一组若不足 `2 + 512` 块就越界。`mark_block` 于是 `bail!("layout overflow")`。

**复现**（`aimgv.sh`）

```
aimg ext4 build ... --size 130M
error: layout overflow: block 33280 >= 33280      # 130MiB = 33280 块，2 组，组 1 需要 514 块
```

2 组可用的最小尺寸 = `32768 + 2 + 512 = 33282` 块 ≈ 130.008 MiB，即 `(128, 130.008] MiB`
这 ~2 MiB 区间整段不可构建（每跨一个组边界就有同样宽的失败带），报错信息也看不出根因。

**影响**：`--size` 由用户给（README 示例就是 `2G`/`3G`），命中边界即失败；`size` 自动推算时也可能命中。

**修复**：像 mke2fs 一样收缩最后一组的 inode table / `inodes_count`：
`last_group_blocks < needed → blocks -= last_group_blocks`（见 `ext4_utils.cpp:135-138`），
或先校验尺寸并给出可读错误（最小可用尺寸 = groups × (2+512) × bs）。

---

### M4. depth-1 extent 树的叶子节点把 `eh_max` 写成 4（实际可放 340 项），且 `i_blocks` 不计索引块

**位置**：`src/ext4/disk.rs:460-476`（`write_leaf_node_raw` 固定 `put16(b,4,4)`）、
`src/ext4/write.rs:293-298` / `327-332`（外置索引节点）、`src/ext4/write.rs:291` / `325`（`i_blocks`）

**问题**
`build_extent_tree` 在 extents > 4 时走 depth-1：为每 340 个 extent 分配一个 4 KiB 叶子块，
但复用了面向 `i_block`（60B，4 项）的 `write_leaf_node_raw`，于是叶子块头部写 `eh_max = 4`
却携带多达 340 条 `eh_entries`。内核 `ext4_ext_check_header()` 会校验
`eh_entries > eh_max` → `invalid eh_entries` → `EFSCORRUPTED`（挂载/读取失败）。
另外 `inode.blocks = nblk * bs/512` 没把索引块算进去，e2fsck 会报
`inode N, i_blocks is X, should be X+8k`（已知 bug 表里只修了 xattr 块这一半）。
触发条件：单文件 > 4 个 extent。因为每次 `alloc_contig` 最多切 32768 块，实际需要 **> 512 MiB 的单文件**；
`build_extent_tree` 还会在 > 3 个叶子时 `bail!("too many extents for depth-1 tree")`（约 > 4 GiB）。

**修复**

```rust
fn write_leaf_node_raw(b: &mut [u8], extents: &[ExtentLeaf], max: u16) { ... put16(b, 4, max); ... }
// 根节点传 4，外置叶子块传 ((bs - 12) / 12) as u16
```
并在分配索引块后 `inode.blocks += (bs/512) * 索引块数`；再往上支持 depth 2。

---

### M5. `sparse to_raw` 不校验 chunk 尺寸：`total_sz = 0` 的 chunk 造成死循环/写爆磁盘

**位置**：`src/sparse.rs:139-184`（展开循环），对照 `src/sparse.rs:97-110`（`parse()` 有校验但 `to_raw` 没用）

**问题**：`to_raw` 每轮只做 `pos += total_sz`，从不校验 `total_sz`：

```rust
while pos + header.chunk_hdr_sz as u64 <= len {
    ...
    match ty { CHUNK_TYPE_RAW => { let mut left = chunk_sz * blk_sz; ...写... }  // 不看 total_sz
               CHUNK_TYPE_DONT_CARE => { ...写 chunk_sz 个块... } }
    blocks += chunk_sz; pos += total_sz;      // total_sz=0 → pos 不前进
}
```

构造 `CHUNK_TYPE_DONT_CARE, chunk_sz = 0xFFFFFFFF, total_sz = 0` 的稀疏镜像：`pos` 永不前进，
循环每次都往输出写 ~16 TiB 的 0（实际表现为卡死 + 磁盘写满）；RAW 变体则会无限重放同一段数据。
`sparse info`（走 `parse`）对 DONT_CARE 也不校验尺寸，只按 `total_chunks` 计数，所以不会先报错。

**修复**：`to_raw` 复用 `parse()` 的校验，并强制
`total_sz >= chunk_hdr_sz`、`pos` 单调推进（`if total_sz == 0 { bail!() }`）。

---

### M6. `boot` 解析对截断镜像直接 panic（越界索引），而不是返回错误

**位置**：`src/boot.rs:158-160`（`cstr(&data[608..1632])`）、`src/boot.rs:257-261` + `95`（`u32at(data, 2076)`）

**问题**：`parse()` 只检查 `data.len() >= 8`，随后无条件按 v0 头部布局切片；`parse_vendor()` 在读
`tags_addr/name/dtb_addr` 之后才检查 `data.len() < 2112`（顺序颠倒）。

**复现**（`aimgv6.sh`，1000 字节文件 + 合法 `page_size`）

```
thread 'main' panicked at src/boot.rs:160:38:
range end index 1632 out of range for slice of length 1000        (exit 101)
thread 'main' panicked at src/boot.rs:95:25:
index out of bounds: the len is 1000 but the index is 2076        (exit 101)
```

**影响**：OTA 半包/坏块/被截断的 boot 分区会让 CLI panic（脚本批量处理时整批中断），而不是清晰报错。

**修复**：解析前按版本要求长度（v0: 1632、v1: 1648、v2: 1660、v3: 1580、v4: 1584、vendor v3: 2112、v4: 2128），
把 vendor 的 `if data.len() < 2112` 提到读取字段之前。

---

## Low

### L1. `bsdiff_apply` 边界检查不足（`ctrl` 段只需 8 字节却读 24 字节；负 x/z 可致 `usize` 回绕）

**位置**：`src/payload.rs:578-613`

```rust
if cp + 8 > ctrl.len() { bail!("bsdiff ctrl truncated"); }
let x = offtin(&ctrl[cp..cp+8]);
let y = offtin(&ctrl[cp + 8..cp + 16]);       // 需要 cp+24
let z = offtin(&ctrl[cp + 16..cp + 24]);
...
dp += x as usize;  ep += y as usize;  oldpos += x + z;
```
`ctrl.len() == cp+10` 时 panic；`x/y/z` 为负（`offtin` 允许符号位）时 `as usize` 变成巨值，
`dp/ep` 回绕后后续 bounds check 失效。`new_size` 也直接 `vec![0u8; new_size as usize]`（OOM）。

**修复**：`if cp + 24 > ctrl.len()`；`if x < 0 || y < 0 || z < 0 { bail!() }`；`new_size` 上限校验。

### L2. `payload` 多处按文件里的 u64 直接分配

**位置**：`src/payload.rs:374`（`vec![0u8; manifest_size]`）、`416`（`vec![0u8; len]`）、`423-426`（`Vec::with_capacity(total)`）

**问题**：`manifest_size`、`data_length`、`num_blocks*block_size` 都来自（可被篡改的）文件，
无上限 → 一个 24 字节声明就能触发数十 GB 分配/OOM。
**修复**：用文件实际长度做上界（`manifest_size <= file_len - 24` 等），并对单 op 设上限。

### L3. `superimg` 对畸形 metadata 会 panic（索引直接下标）

**位置**：`src/superimg.rs:451`（`meta.extents[p.first_extent_index + i]`，而 `partition_size` 用的是 `.get()`）、
`374-379`（`Vec::with_capacity` 直接用 `num_chunks`）、`214-216`（不校验 `major_version == 10`）

**问题**：`first_extent_index` 越界 → panic（同文件其它地方都做了边界检查，风格不一致）；
未校验 major 版本，未来格式会被当成 v10 静默解析。
**修复**：`let e = meta.extents.get(...).ok_or_else(...)?;`，并加 `if meta.major != 10 { bail!() }`。

### L4. `raw_to_sparse` 每个块只 `read()` 一次，短读会静默错位

**位置**：`src/sparse.rs:307-319`

```rust
let n = src.read(&mut buf)?;      // 不保证填满
if n < buf.len() { for b in buf[n..].iter_mut() { *b = 0; } }   // 短读被当成“文件尾”
cw.block(&buf)?; left -= 1;       // 但文件位置已前移 n（非整块），后续块全部错位
```
本地 NTFS 通常一次读满，但网络盘/Dokan/管道输入会短读 → 输出镜像静默错位。
**修复**：`src.read_exact(&mut buf)`（末尾用 `take`/手写循环处理最后一个不足块）。

### L5. `reserved_blocks` 只减 `s_free_blocks_count`，从不写 `s_r_blocks_count`

**位置**：`src/ext4/write.rs:467`、`src/ext4/disk.rs:127-169`

`free_blocks = total_free_blocks.saturating_sub(opt.reserved_blocks)`，但 `Superblock::serialize`
没有写 offset 8（`s_r_blocks_count`），于是保留块“既不在 free 也不在 reserved”，
e2fsck 会报 `Free blocks count wrong`。CLI 未暴露该字段（默认 0），仅库调用可触发。
**修复**：`Superblock` 增加 `reserved_blocks` 字段并写到 offset 8，同时不要从 free 里扣。

### L6. boot v0–v2 不写 `id[8]`（SHA1），空 ramdisk/second 仍写加载地址

**位置**：`src/boot.rs:366-389`

AOSP `mkbootimg.py:229-241` 会计算 `sha1(kernel|ramdisk|second[|recovery_dtbo][|dtb])` 写入 576..608，
并对空 ramdisk/second 写 0 地址（`mkbootimg.py:199-202`）。本实现两者都留 0/写死地址，
导致 `unpack → pack` 不是字节恒等（旧 bootloader 依赖 `id` 校验时会失败）。
**修复**：按版本计算 SHA1 并写 `id`；空段地址写 0。

### L7. `ext4 extract` 丢根目录元数据 / 硬链接 / 目录符号链接方向

**位置**：`src/ext4/read.rs:308-321`（`if vpath != "/"` 才入库）、`src/ext4/read.rs:434-437`（Windows 一律 `symlink_file`）

* 根目录 `/` 的 uid/gid/mode/`security.selinux` 从不写入 `meta.txt`，回包时 `build_tree` 用
  `default_entry("/")`（0:0 0755 + `u:object_r:unlabeled:s0`）→ 根上下文（如 `u:object_r:rootfs:s0`）丢失。
* 硬链接：同一 inode 被当作独立文件解出、各自 1 条记录，`i_links_count` 不保留 → 回包后变成多份副本。
* Windows 上所有符号链接都用 `symlink_file`，指向目录的链接语义错误（且通常因权限退化成普通文件）。

**修复**：把根目录条目一并写入（`build_tree` 已优先取 `meta.get("/")`）；按 inode 去重以保留硬链接；
按 `readlink` 目标类型/`S_IFDIR` 选择 `symlink_dir`。

---

## 检查过、确认无问题的点（避免重复报告）

1. **LP metadata 布局**：geometry 在 4096、backup 在 8192、primary metadata 在 12288、backup 在
   `12288 + max_size*slots`，与 `vendor/lpunpack_and_lpmake/lib/include/liblp/metadata_format.h` 及
   `lpdump` 输出一致；实测 `super make` 产出的 `first_logical_sector=2048`、extent/group 表可被自家 dump/unpack 正确回读。
2. **sparse 解析**：chunk 头 12 字节、RAW/FILL/CRC32 的 `total_sz` 校验、多卷合并、raw→sparse 尾部补齐，
   往返用例通过；`raw_to_sparse` 的 DONT_CARE 合并（-4096 块上限）与 img2simg 行为一致。
3. **ibody xattr 读取（已知 bug 2 的修复）**：用 `debugfs -w -R "ea_set /big.bin security.selinux ..."`
   在真实 mke2fs ext4 上写入内联 xattr（`File ACL: 0`、`extra_isize=32`），`aimg ext4 extract`
   正确读出 `u:object_r:test_ibody:s0`；value 偏移以 entries 起点为基准的实现是对的。
4. **外部 xattr block**：`h_hash`/`e_hash`/值 4 字节补齐参与哈希的实现被 `e2fsck 1.47.0` 接受
   （单组镜像 `debugfs ea_list` 显示 `security.selinux`，`e2fsck -fn` 干净）。
5. **真实 mke2fs ext4 读取**：`metadata_csum + 64bit + flex_bg + extents + htree` 的镜像（305 条目、
   含 300 个 htree 目录项、300 KB 文件）解包后内容逐字节一致。
6. **单组 ext4 构建**：32 MiB / 64 MiB 镜像 `e2fsck -fn` exit 0（`16/8192 files, 577/8192 blocks`），
   说明 C1/C2 之外的元数据（itable_unused=0、组 0 位图占用、`i_blocks` 含 xattr 块）没有问题。
7. **payload full 包**：`payload make` → `payload extract --verify` 往返一致（含 `--chunk-size` 边界）；
   protobuf 手写编解码字段号与 `update_metadata.proto` 一致。
8. **boot 解析/回包**：v0–v4 与 vendor_boot v3/v4 的段顺序（kernel/ramdisk/second/recovery_dtbo/dtb/
   signature、header/vendor_ramdisk/dtb/table/bootconfig）、AVB footer 裁剪、vendor_boot 表项 108 字节布局均正确。
9. **dat**：transfer.list v1–v4 头部（版本 / 总块数 / v≥2 的 stash 两行）解析正确，
   `new`/`zero`/`erase` 的合并与 `img2sdat` 产物可互相回读，`rangeset` 计数校验正常
   （差分命令 `bsdiff`/`move`/`stash` 的真实语法问题见 H5）。
10. **perm 语义**：`fs_config` 精确匹配→最近祖先继承、`file_contexts` 字面前缀最长者胜（同长取后者），
    与 `libselinux` 的 stem 规则一致；`meta.txt` TSV 往返无损。

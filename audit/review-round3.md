# 修复状态（第三轮，2026-11-15 晚）

| 编号 | 状态 |
|---|---|
| 截断 v3/v4 boot panic | 已修（v>=3 字段区间独立） |
| protobuf varint 溢出 panic | 已修（checked_add） |
| bpg/ipg=0 除零 panic | 已修（超级块校验） |
| --size 乘法溢出 panic | 已修（checked_mul） |
| 每文件 sync_all（50 ms/文件） | 已修，100 文件解包 6 ms |
| extract O(n^2) | 已修（push + sort + dedup） |
| extra_blocks 全量驻留 | 已修（xattr/目录块即时写盘） |
| auto-size 不看 inode 数 | 已修（按组数兜底），20k 文件无 --size 构建 + fsck 通过 |
| LP tables/entry_size 无上限 | 已修 |
| minor_version 强制 --old | 已修（按 op 判定） |
| properties 文件名 | 已修（payload.bin.properties） |
| meta.txt 被打进镜像 | 已修（from_dir 跳过） |
| ext4 sparse 输入 | 已修（自动展开） |
| 64K 块 bpg 溢出 | 已修（4 GiB 组上限） |
| 其余 Low（死代码、ibody、大小写/junction、Windows 硬链接、dat gap 裁剪） | 未改 |

---

# AndroidImg_Rust (`aimg`) — Code Review Round 3

日期：2026-11-15（晚）
范围：`src/`（13 个 `*.rs`，250 KB）、`tests/e2e.rs`、`python/pyaimg` + `python/tests`、`README.md`、`Cargo.toml`
前序：`audit/findings.md`（第一轮 C/H/M/L）、`audit/review-2026-11-15.md`（第二轮 N1–N17）
性质：**第三轮全量复审**。除静态审读外，本轮**用本机 debug 二进制实测**了 15 处行为（含 5 个 panic）。

---

## 0. 结论摘要

| 项 | 结果 |
|---|---|
| `cargo build --all-targets`（dev，rustc 1.99.1） | 通过，**1 warning**（`src/ext4/write.rs:11` 未使用 import，第二轮报告称 0 warning 已失效） |
| `cargo test` | **21 unit + 2 e2e 全过**（0.48 s / 38.98 s） |
| 本轮新发现 | **High 7 / Medium 18 / Low 21，合计 46 项** |
| 其中实测确认（非静态推断） | 15 项，含 **5 个可稳定复现的 panic**、2 个可稳定复现的性能塌陷 |
| 最该先修 | **R3-H1/H2/H3/H4**（4 处 panic：截断 boot 头、protobuf varint、`s_blocks_per_group=0`、`s_inodes_per_group=0`）、**R3-H7**（每文件一次 `fsync`，20k 文件解包 >12 min）、**R3-H10**（auto-size 不看 inode 数，忘给 `--size` 直接构建失败） |
| 第二轮遗留（本轮复核仍未修） | N6.1 BSDF2、N6.2 properties 命名、N8 ibody xattr、N9（只压警告未解决）、N12 死代码、N13 大小写/junction、N14 硬链接、N16 溢出、N17 文档过期 |

**实测环境**：Windows 11 + rustc/cargo 1.99.1（stable-x86_64-pc-windows-msvc），
`F:\DefaultProjects\AndroidImg_Rust\target\debug\aimg.exe`（本轮 `cargo build` 产物，与当前源码一致）。
构建/测试输出见 §1。**本机无 e2fsprogs、无 WSL、内存紧张**，因此 `e2fsck` 类校验未做（见 §6）。
副作用：`cargo build` 未改动 `Cargo.lock`（本轮 `Cargo.toml`/`Cargo.lock` 一致）。

---

## 1. 实测记录（可复跑）

```
$ cargo build --all-targets
warning: unused import: `self`
  --> src\ext4\write.rs:11:15
   11 | use std::fs::{self, File};
warning: `aimg` (lib) generated 1 warning
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 50.33s

$ cargo test
running 21 tests ... test result: ok. 21 passed; 0 failed
running 2 tests  ... test result: ok. 2 passed; 0 failed   (38.98s)
```

| # | 实验 | 命令要点 | 结果 |
|---|---|---|---|
| E1 | 截断 v3 boot | 1584 B 文件，magic + `header_version=3` | **panic** `boot.rs:177:38 range end index 1632 out of range for slice of length 1584`（exit 101） |
| E2 | 畸形 payload manifest | field 13(wire 2) + varint 长度 `2^64-1` | **panic** `payload.rs:120:12 attempt to add with overflow`（exit 101） |
| E3 | `s_blocks_per_group=0` | 合法镜像 offset `1024+32` 写 0 | **panic** `disk.rs:120:45 attempt to divide by zero`（exit 101） |
| E4 | `s_inodes_per_group=0` | offset `1024+40` 写 0 | **panic** `read.rs:52:21 attempt to divide by zero`（exit 101） |
| E5 | `--size 300000000000G` | `ext4 build` | **panic** `util.rs:110:8 attempt to multiply with overflow`（exit 101） |
| E6 | 解包吞吐（20/40/80 文件） | `ext4 extract` ×3 | 1174 / 2088 / 4089 ms → **51–59 ms/文件**（线性，与文件大小无关） |
| E7 | 解包 500 文件 | `ext4 extract` | **63.35 s** |
| E8 | 解包 20000 文件 | `ext4 extract`（1 GiB 镜像） | **>12 min 未完成**（已中断） |
| E9 | 构建 20000 文件（`--size 1G`） | `Start-Process` + 30 ms 采样 PeakWorkingSet | exit 0，**峰值 RSS 107.7 MiB** |
| E10 | 构建 20000 文件（不给 `--size`） | `ext4 build --dir many` | **失败** `error: inode table exhausted` |
| E11 | 1 KiB 块 + 单文件 8 MiB | `--block-size 1024 --size 512M` | exit 0，`ext4 info` 正常，解包 MD5 一致（**N1 确认已修**） |
| E12 | `super make --alignment 4096` | `--size 8M` | `first_logical_sector=536`（274432 B，**N3 确认已修**） |
| E13 | A/B super 双槽解包 | `system_a` + `system_b`，`--slot 0` | 只产出 `system.img`，无覆盖（**N4 确认已修**） |
| E14 | `payload make` 产物命名 | `--out payload.bin` | 生成 `payload.bin` + **`payload.properties`**（应为 `payload_properties.txt`） |
| E15 | minor_version=2 且全 REPLACE | 手工把 manifest 加 `field12=2` | `payload extract` 报 **`delta payload: -old required for system`** |
| E16 | `--block-size` 2K/8K/16K/32K/64K | 48 MiB 镜像 | 全部构建成功（64K 时 `rec_len=0` 特殊编码生效） |
| E17 | `ext4 info` 稀疏输入 | `.simg` 直接喂 ext4 | `error: bad ext4 magic 0xff2`（未识别 sparse） |
| E18 | 解包目录重建（不给 `--meta`） | `ext4 build --dir <extract_out>` | 镜像里**含 `/meta.txt`** |

---

## 2. High

### R3-H1 截断的 boot v3/v4 镜像直接 panic（实测 E1）

**位置**：`src/boot.rs:144-154`（版本→所需长度）、`src/boot.rs:177`

```rust
144:        let need = match header_version {
145:            0 => 1632,
...
148:            3 => 1580,
149:            4 => 1584,
...
152:        if data.len() < need { bail!(...) }
...
177:            extra_cmdline: cstr(&data[608..1632]),   // ← 对所有版本无条件切 1632
```

**证据**（E1）：1584 字节的 v3 镜像通过 `need=1580` 校验后，在 177 行切 `data[608..1632]`：

```
thread 'main' panicked at src\boot.rs:177:38:
range end index 1632 out of range for slice of length 1584
exit=101
```

**影响**：任何被截断/伪造的 v3/v4 boot、vendor_boot 之外的小镜像都会让 `aimg boot info|unpack` 直接 abort（不是 `Err`，
`main()` 的 `error: {:#}` 路径走不到；Windows 上是弹窗式 abort，CI 里是 core dump）。第二轮 N6/L 系列修过
"截断 boot 镜像 panic"，但只覆盖了 `take()` 路径，**header 字段切片这条没修**。

**修法**：改成按版本取，或统一安全切片：

```rust
let extra = if header_version < 3 { cstr(&data[608..1632]) } else { String::new() };
// 或
fn slice_or_empty(d: &[u8], a: usize, b: usize) -> &[u8] { d.get(a..b).unwrap_or(&[]) }
```
并在 `need` 里对 v3/v4 也用 `max(1632)` 之外的方式表达（v3/v4 只有 1584 字节头，不必再读 extra_cmdline）。

---

### R3-H2 protobuf 长度字段溢出 → panic（实测 E2）

**位置**：`src/payload.rs:118-126`

```rust
118:    fn bytes(&mut self) -> Result<&'a [u8]> {
119:        let n = self.varint()? as usize;
120:        if self.p + n > self.d.len() {        // ← 无 checked_add
121:            bail!("truncated protobuf field");
122:        }
123:        let s = &self.d[self.p..self.p + n];
```

**证据**（E2）：manifest 里塞 `field 13, wire 2` + 长度 varint `0xFFFFFFFFFFFFFFFF`：

```
thread 'main' panicked at src\payload.rs:120:12: attempt to add with overflow
exit=101
```

debug 下是 add overflow；release 下 `self.p + n` 回绕成小值 → 通过 121 行检查 → 123 行切片 panic
（`range end index ... out of range`）。两条路径都是 panic。

**影响**：`payload info` / `payload extract` 对任何畸形/损坏的 payload.bin 都 abort。OTA 包是外部输入，
这是最典型的"必须在边界上返回 Err"的位置。

**修法**：
```rust
let n = usize::try_from(self.varint()?).map_err(|_| anyhow!("field length too large"))?;
let end = self.p.checked_add(n).filter(|e| *e <= self.d.len())
    .ok_or_else(|| anyhow!("truncated protobuf field"))?;
let s = &self.d[self.p..end];
self.p = end;
```
顺带 `skip()` 的 wire 1/5 分支也要 `checked_add`（当前 `self.p += 8` 可越界后静默截断）。

---

### R3-H3 ext4 头字段未校验：两处除零 panic（实测 E3/E4）

**位置**：`src/ext4/disk.rs:118-122`、`src/ext4/read.rs:47-53`

```rust
// disk.rs
118:    pub fn group_count(&self) -> u32 {
119:        let first = self.first_data_block as u64;
120:        let n = (self.blocks_count - first).div_ceil(self.blocks_per_group as u64);  // bpg=0 → panic
```
```rust
// read.rs
51:        let idx = ino - 1;
52:        let group = idx / self.sb.inodes_per_group;      // ipg=0 → panic
```

**证据**：E3 → `disk.rs:120:45 attempt to divide by zero`；E4 → `read.rs:52:21 attempt to divide by zero`。
另外 `blocks_count < first_data_block` 时 120 行的减法在 debug 下会 underflow panic（release 回绕成巨大值，
随后 `for i in 0..group_count` 疯狂读 GDT）。

**影响**：任何损坏/裁剪过的 ext4（或非 ext4 但 1024 偏移处恰好 `0xEF53` 的数据）都能让
`ext4 info|ls|extract` abort。`Superblock::parse`（disk.rs:75-112）目前**只校验了魔数和 log_block_size**。

**修法**：在 `Superblock::parse` 末尾集中校验，失败即 `bail!`：

```rust
if self.blocks_per_group == 0 || self.inodes_per_group == 0 { bail!("bad geometry: bpg/ipg = 0"); }
if self.blocks_count <= self.first_data_block as u64 { bail!("bad blocks_count"); }
if self.block_size.count_ones() != 1 || !(1024..=65536).contains(&self.block_size) { bail!("bad block size"); }
if !(128..=4096).contains(&(self.inode_size as u32)) || self.inode_size % 4 != 0 { bail!("bad inode_size"); }
if !matches!(self.desc_size, 32 | 64) { bail!("unsupported desc_size {}", self.desc_size); }  // 见 R3-M10
if self.inodes_count == 0 { bail!("bad inodes_count"); }
```

---

### R3-H4 `read_data` 的空洞填充按 inode.size 无上限分配 → OOM

**位置**：`src/ext4/read.rs:174-223`（`read_data`）、`src/ext4/read.rs:199-206`

```rust
186:        let mut out: Vec<u8> = Vec::with_capacity(size.min(64 << 20));
...
199:            if logical > next_logical {
200:                let gap = (logical - next_logical) * bs;
201:                let n = gap.min((size.saturating_sub(out.len())) as u64) as usize;
202:                out.resize(out.len() + n, 0);            // ← 最多可扩到 size 字节
```

`size = inode.size`（`i_size_high` 参与，最大 ~2^48）。一个 4 字节文件，只要 extent 的 `ee_block`
写成 `0x00FFFFFF`、`i_size` 写成 4 GiB，`read_data` 就会老实分配 4 GiB 全零。`read_dir` 走的也是
`read_data`，所以**目录 inode 同样可被利用**。

**证据**：静态确认（未实跑，避免本机 OOM）；`Vec::with_capacity(min(64MiB))` 只限制了初始容量，`resize` 不设上限。
第二轮 L2 给 manifest/sig/blob 都加了上限，但这条路径漏了。

**影响**：`aimg ext4 extract` 处理不受信任/损坏镜像时被打爆内存；本机（8 GB 物理内存、常年只剩数百 MB）
一次就够失败。

**修法**：三条一起做——
1. `read_data` 加 `if logical > next_logical + (1 << 20) { bail!("implausible hole") }`（同 e2fsprogs 的
   sparse 上限思路），或至少 `if n as u64 > (64 << 20) { bail!("hole too large") }`；
2. 目录读取改用 `write_file_data` 的流式路径（目录本身不该 > 几 MiB），或对 dir 用 `bs` 逐块解析；
3. `write_file_data`（read.rs:226-271）同样按 `size` 预校验：`if size > image_len { bail! }`。

---

### R3-H5 `super` metadata 的 `tables_size` 无上限分配

**位置**：`src/superimg.rs:229-231`

```rust
229:    let tables_size = u32at(&hdr, 44) as usize;
230:    let mut tables = vec![0u8; tables_size];        // ← 最大 4 GiB
231:    read_at(f, off + header_size as u64, &mut tables)?;
```

**证据**：`header_size` 有 128..4096 的上限（220 行），但 `tables_size` 完全没有；它只是被
`sha256(&tables)` 校验，校验发生在**分配之后**。

**影响**：一个 5 GiB 的垃圾 super.img（或手工改掉 `header_checksum` 之前的 `tables_size`）就能让
`super dump/unpack` 申请 4 GiB。`off + header_size + tables_size` 也没有与文件长度比较。

**修法**：
```rust
let file_len = f.metadata()?.len();
if tables_size as u64 > geo.metadata_max_size as u64
   || (off + header_size as u64).saturating_add(tables_size as u64) > file_len {
    bail!("metadata tables ({} bytes) exceed metadata_max_size/file", tables_size);
}
```
（`metadata_max_size` 就是这块区域的物理上限，用它当 cap 最自然。）

---

### R3-H6 `super` 表描述符 `entry_size` 未校验 → 巨大循环 / 越界

**位置**：`src/superimg.rs:236-302`

```rust
236:    let d = |o: usize| (u32at(&hdr, o), u32at(&hdr, o + 4), u32at(&hdr, o + 8));
237:    let pd = d(80);
...
244:    for i in 0..pd.1 as usize {                 // pd.1 最大 4e9
245:        let o = pd.0 as usize + i * pd.2 as usize;   // pd.2 == 0 → o 恒定
246:        if o + PART_ENTRY_SIZE > tables.len() { bail!(...) }
```

`pd.2`（entry size）、`ed.2`、`gd.2`、`bd.2` 都直接来自文件，**没有任何合法值校验**（应为 52/24/48/64）。
若 `pd.2 == 0`、`pd.0` 有效，则 `o` 恒定、`246` 行永远通过 → 循环 2^32 次、每次 push 一个 `Partition`
（含 String）→ 数 GB 内存 + 长时间挂死。若 `pd.2` 是垃圾值，则各表按错误步长解析出无意义条目
（后续 `unpack` 会按这些条目算 `src_off/len`，`copy_range` 到 EOF 报错，但 `dump` 会输出完全错误的信息）。

**影响**：不受信任 super.img 的 DoS；且"能解析但不报错"的错位解析比 panic 更危险（下游 `super unpack`
会按错误 extent 拷数据）。

**修法**：
```rust
fn check_desc(name: &str, d: (u32,u32,u32), want: u32, tables_len: usize) -> Result<()> {
    if d.2 != want { bail!("{} entry_size {} != {}", name, d.2, want); }
    let end = (d.0 as usize).checked_add(d.1 as usize * want as usize);
    if end.map_or(true, |e| e > tables_len) { bail!("{} table overflows metadata", name); }
    Ok(())
}
```
对 block_devices 允许 60/64 两种（见 `283-302` 行现有兼容逻辑，保留但限定为 `{60,64}`）。

---

### R3-H7 `ext4 extract` 每个文件一次 `sync_all()` → 解包吞吐塌陷（实测 E6–E8）

**位置**：`src/ext4/read.rs:226-271`，罪魁在 `:269`

```rust
226:    pub fn write_file_data(&mut self, inode: &Inode, path: &Path) -> Result<()> {
230:        let mut out = File::create(path)?;
...
269:        out.sync_all()?;              // ← 每个文件一次 fsync
270:        Ok(())
```

**证据**：

| 文件数 | 实测耗时 | 折算 |
|---|---|---|
| 20 | 1174 ms | 58.7 ms/文件 |
| 40 | 2088 ms | 52.2 ms/文件 |
| 80 | 4089 ms | 51.1 ms/文件 |
| 500 | 63.35 s | 126 ms/文件（叠加 R3-H8） |
| 20000 | >12 min（中断） | 预计 40 min+ |

4 字节文件解包 51 ms，CPU 工作只有微秒级 → 成本全在 `flush+fsync`（本机 NTFS 单次 sync ≈ 50 ms）。

**影响**：真实 `system.img`（约 2–4 万文件）解包 **25–40 分钟**，而且在 SSD 上也不改善多少（fsync 是
协议开销）。README §4 的主流程（payload → super → ext4 extract → 改 → build）因此实际上不可用。
`sync_all` 在这里毫无价值：解包产物是普通文件，进程正常退出时 OS 已经保证数据可见性；断电丢半个
extract 目录也不会比现在更糟。

**修法**：删掉 `:269` 的 `out.sync_all()`（或改成 `out.flush()?` 即可——`File` 的 `write_all` 本来就在
`BufWriter` 之下才需要 flush，这里没有 `BufWriter`，直接删）。若要保留"落盘确认"，放到
`extract_tree` 末尾做一次（遍历目录 fsync 也不要，直接不管）。

---

### R3-H8 `MetaDb::insert` 线性查重 → `extract_tree` 整体 O(n²)（实测 E7/E8）

**位置**：`src/perm.rs:545-551`（`insert`）、`src/ext4/read.rs:423-437`（每个条目调一次）

```rust
545:    pub fn insert(&mut self, e: Entry) {
546:        if let Some(pos) = self.entries.iter().position(|x| x.path == e.path) {   // O(n)
547:            self.entries[pos] = e;
548:        } else {
549:            self.entries.push(e);
550:        }
551:    }
```
```rust
// read.rs:424
                    db.insert(Entry { ... });
```

**证据**：E7（500 文件 63.35 s，其中约 63 s−500×51 ms≈38 s 是 O(n²) 查重 + 每步 `String` 比较）；
E8（20000 文件 12 min 未完成）。500² / 2 = 12.5 万次比较本不该是 38 s，但 debug 下每次比较都是
`String == &str` + `iter().position()` 迭代器开销，加上此处 `Entry` 含 4 个 `String`。

**影响**：与 R3-H7 叠加，把"解包"从分钟级推到小时级；`perm apply`/`by_path` 也跟着遭殃。
第一轮修过 `tree_rec` 的 O(n²)（N16），这里是同一类问题在数据层的翻版。

**修法**：`extract_tree` 里根本不需要 `insert`（不会重复：队列里每个 virtual path 只出现一次）。
两处一起改最省事：
1. `extract_tree` 改成 `db.entries.push(...)`，末尾 `sort_by(path)`（现在末尾已有 sort，见 read.rs:455）；
2. `MetaDb` 增加 `index: HashMap<String, usize>`（`insert` 时维护，`get`/`insert` 都 O(1)），
   `save/parse/load` 后重建索引。

---

### R3-H9 构建器仍把"每个 inode 4 KiB xattr 块 + 每个目录块"全量驻留内存（实测 E9）

**位置**：`src/ext4/write.rs:306`、`:488-515`、`:521-523`

```rust
306:    let mut extra_blocks: Vec<(u64, Vec<u8>)> = Vec::new();
...
508:        if !xattrs.is_empty() {
509:            let blk = alloc_contig(1, &mut used, &mut cursor)?;
510:            let bytes = encode_xattr_entries(&xattrs, bs as usize)?;   // bs 字节
511:            extra_blocks.push((blk, bytes));                            // ← 每个 inode 一份
...
521:    for (blk, data) in &extra_blocks {                                  // 全部留到最后才写
522:        write_at(&mut f, blk * bs as u64, data)?;
523:    }
```

**证据**（E9）：20000 个 8 字节文件 + `--size 1G` → **峰值 RSS 107.7 MiB**。理论值：
20001 inode × 4096 B ≈ 78 MiB（xattr 块）+ 目录块 + `inode_blobs`（20001 × 256 B ≈ 5 MiB）。
文件数据已经是流式的（`src` → 1 MiB `buf` → `write_at`），所以这 100 MiB **几乎全是 `extra_blocks`**。

**影响**：README §8.1 声称"payload/镜像构建全量进内存 → 已修（160 MiB 文件 RSS 5.8 MiB）"，那个结论
只对"单文件很大"成立。真实 Android 树是"文件不大但 inode 极多"：`system.img` 约 3–5 万 inode →
150–200 MiB；`vendor`+`product`+`system_ext` 一起上 10 万 inode → 400 MiB。本机（可用内存常年数百 MB）
构建 3 GB 镜像仍会失败，而失败点从"文件数据"搬到了"每个 inode 的 4 KiB xattr 块"。

另外 `inode_blobs: HashMap<u32, Vec<u8>>`（`:305`）也是全量驻留，量级 256 B × inode 数（次要）。

**修法**：`f` 在 `:304` 就已经创建好了，`extra_blocks` 没有任何"必须最后写"的理由（块号已通过 `used`
标记占用）。改成**分配即写**：

```rust
// 目录/文件 extent 节点/long symlink/xattr 块，统统在 alloc 之后立刻落盘
write_at(&mut f, blk * bs as u64, &bytes)?;
```
`inode_blobs` 同理可在算完后立刻写 inode 表（但 inode 表偏移依赖最终 `ino`，需要两趟：先分配 ino
（已有），再逐节点写）。最小改动版：去掉 `extra_blocks`，改为 `Vec<(u64,u32)>` 记录"待写块 + 长度"
或直接就地写；`inode_blobs` 保留（256 B × n 可接受），峰值降到 O(目录块单块 + inode 表)。

---

### R3-H10 auto-size 完全不看 inode 数量 → 忘给 `--size` 直接构建失败（实测 E10）

**位置**：`src/ext4/write.rs:155-165`、`:173`、`:252-262`

```rust
155:    let data_bytes: u64 = nodes.iter().filter(|n| n.kind == Kind::File)
157:        .map(|n| n.entry.size).sum();
160:    let mut size = if opt.size > 0 { opt.size } else {
163:        let raw = data_bytes + data_bytes / 4 + 32 * 1024 * 1024;
164:        align_up(raw.max(16 * 1024 * 1024), 1024 * 1024)
165:    };
...
173:    let mut ipg: u32 = if bs >= 4096 { 8192 } else { 2048 };
...
256:            if next_ino >= total_inodes { bail!("inode table exhausted"); }
```

**证据**（E10）：20000 个 8 字节文件的树，`aimg ext4 build --dir many --out auto.img` →
`error: inode table exhausted`。因为 `data_bytes`≈160 KB → `size=32 MiB` → `groups=1`、`ipg=8192`
→ `total_inodes=8192 < 20001`。错误信息**没有提示 `--size`，也没有 `--inodes` 可用**。
同一棵树给 `--size 1G` 就成功（E9）。

**影响**：README §4 第 4 步的典型写法是 `ext4 build --dir system_root/ --out parts/system.raw --meta ... --size 3G`
（带了 size），但 §3.6 的用法说明把 `--size` 写成可选；一旦省略（或解包后先小规模试跑），就会撞上
"inode table exhausted" 而完全不知道原因。mke2fs 的做法是 `inode_ratio=16384 B/inode`，且提供 `-N`。

**修法**：
1. `size` 估算里加入 inode 需求：
   ```rust
   let files = nodes.len() as u64;
   let need_inodes = files + files / 4 + 1024;
   let by_inodes = need_inodes * 8 /*每 inode 至少 8 字节余量*/ * 4 ... // 或直接:
   let need_bytes = need_inodes * inode_size as u64 * 2;   // ipg 上限内
   raw = max(raw, need_bytes);
   ```
   简单可靠版：`size = max(size, align_up(need_inodes * inode_size as u64 * 2, 1MiB))`；
2. 新增 `--inodes N`，`ipg` 按 `ceil(N / groups)` 反推（并与 `itable_blocks` 一起重算）；
3. 错误信息补上：`"inode table exhausted: need {} inodes, image has {} (pass --size/--inodes)"`。

---

## 3. Medium

### R3-M1 `payload extract` 对 `minor_version != 0` 一律要求 `--old`（实测 E15）

**位置**：`src/payload.rs:531-543`

```rust
531:        // Base image for delta operations.
532:        let mut old: Option<File> = None;
533:        if payload.manifest.minor_version != 0 {
534:            let dir = opt.old_dir.as_ref()
537:                .with_context(|| format!("delta payload: -old required for {}", p.name))?;
```

**证据**（E15）：把生成的 full payload 的 manifest 追加 `field 12 = 2`（唯一改动），
`payload info` 显示 `minor_version: 2`，`payload extract` 立刻失败：
`error: delta payload: -old required for system`——尽管该分区 **ops 全是 REPLACE**，根本不需要 base。

**影响**：真实 OTA 里 `minor_version=1/2` 的包很常见，其中 `boot`、`vbmeta`、`dtbo` 等分区通常整段
REPLACE；`partial_update=true`/brick 包也常是全量分区。用户被迫准备一份 base 镜像目录（多下载数 GB），
否则连全量分区也拿不到。这与 README §3.4 "增量包必须提供 --old" 的字面描述一致，但语义上过严。

**修法**：把 `old` 变成**按需**打开：遇到 `OP_SOURCE_COPY`/`OP_SOURCE_BSDIFF` 时才 `require` base；
```rust
let need_old = p.ops.iter().any(|o| matches!(o.ty, OP_SOURCE_COPY | OP_SOURCE_BSDIFF | OP_BSDIFF));
if need_old { /* open dir/<name>.img */ }
```

---

### R3-M2 `payload make` 的 properties 文件名错了（实测 E14）

**位置**：`src/payload.rs:869`

```rust
869:    fs::write(out.with_extension("properties"), props)?;
```

**证据**（E14）：`--out payload.bin` → 产出 **`payload.properties`**。Android 的约定名是
**`payload_properties.txt`**（`update_engine_client --payload_metadata=payload_properties.txt`、
`adb sideload` 旁的校验脚本、各类刷机工具都按这个名字找）。README §8.1 也把它写成"生成 `.properties`"，
说明这是有意的简化，但和工具链不兼容。

**修法**：
```rust
let props_path = out.parent().unwrap_or(Path::new(".")).join("payload_properties.txt");
// 若担心覆盖：out.with_file_name(format!("payload_properties.txt"))
```
另外 `FILE_HASH` 现在靠"把整个输出再读一遍"算（`:852-861`），可以和写入合并（边 `io::copy` 边 `update`）。

---

### R3-M3 `meta.txt` 会被打进重建镜像（实测 E18）

**位置**：`src/main.rs:656`（默认落在 out 目录内）、`src/main.rs:676-693`（只压警告）、
`src/perm.rs:354-446`（`from_dir` 照单全收）

```rust
// main.rs:682 —— 只在统计 warning 时跳过
682:                            if vp != "/meta.txt" && !known.contains(&vp) { missing += 1; }
```

**证据**（E18）：`ext4 extract fs20.img --out t1out`（生成 `t1out/meta.txt`）→
`ext4 build --dir t1out --out rb.img --size 32M` → `ext4 ls rb.img` 里出现 **`meta.txt`**。

**影响**：README §4 的"解包 → 改 → 回包"闭环中，若用户按 §3.6 的写法**不加 `--meta`**（"meta.txt 会自动生成"），
`meta.txt` 就会成为系统镜像里的一个垃圾文件。第二轮 N9 只解决了"下次构建报 warning"，实际产物仍然污染。

**修法**：两处都改——(a) `ext4 extract` 默认写 `<out>.meta.txt`（同级），或写 `.aimg/meta.txt`；
(b) `MetaDb::from_dir` 增加跳过列表（`meta.txt`），或 `build_tree` 在 `safe_image_path` 之后过滤掉
basename 为 `meta.txt` 且 size 与 meta 不符的条目。最稳的是 (a) + `build` 侧显式 `--meta` 默认查找。

---

### R3-M4 `ext4` 不识别 sparse 输入（实测 E17）

**位置**：`src/main.rs:613`、`:639`、`:648`（全部直接 `Ext4::open`）

**证据**（E17）：`aimg ext4 info system.simg` → `error: bad ext4 magic 0xff2`（`0xff2` 是 sparse 头里
`file_hdr_sz` 附近的字节，用户完全看不懂）。

**影响**：README §4 第 5 步刚做完 `sparse from-raw`，若回头再 `ext4 info` 就报一个莫名其妙的魔数错误。
第二轮 N5 修了 `super make` 一侧（`superimg.rs:646-655` 现在会 `bail!("...run sparse to-raw first")`），
ext4 侧仍未修。

**修法**：`Ext4::open` 里先探测，再自动展开（复用 `superimg::open_maybe_sparse` 的临时文件方案），
或至少给出 `"input is an Android sparse image; run `aimg sparse to-raw`"`。

---

### R3-M5 `pack_dir_blocks` 每目录算两遍

**位置**：`src/ext4/write.rs:411-412`

```rust
411:        let blocks_needed = pack_dir_blocks(&ents, bs as usize)?.len() as u64;
412:        let packed = pack_dir_blocks(&ents, bs as usize)?;
```

**影响**：每个目录多一次全量打包（含每块的 `vec![0u8; bs]` 分配与 `render_block` 渲染）。
`system.img` 有数千目录 → 数千次冗余的全块分配。纯浪费，且让 R3-H9 的峰值翻倍（两份额临时驻留）。

**修法**：`let packed = pack_dir_blocks(...)?; let blocks_needed = packed.len() as u64;`

---

### R3-M6 `--block-size` 允许 8192…65536，但产物在 Android 上不可挂载（实测 E16）

**位置**：`src/main.rs:276-277`（`default_value="4096"`，无范围限制）、`src/ext4/write.rs:87-90`

**证据**（E16）：2K/8K/16K/32K/64K 全部构建成功并能自读。但 Android 内核 `EXT4_MAX_BLOCK_SIZE` 一般是
4096（只有 arm64 4K-page 的内核开 `CONFIG_EXT4_FS` 的 64K 支持、以及部分 16K 设备例外）。
README §3.6 明写"构建器实现的是 ext4 的一个可靠子集：**4 KiB 块**"。

**影响**：用户传 `--block-size 65536` 得到"能构建、能自读、设备挂载失败"的镜像，且没有任何提示。

**修法**：`if ![1024u32, 2048, 4096].contains(&bs) { eprintln!("warning: block size {} is not mountable on most Android kernels", bs) }`，
或直接把 CLI 值钳到 `{1024,2048,4096}`（想保留实验能力就加 `--force-block-size`）。

---

### R3-M7 每组都全表扫 `nodes` 统计 `used_dirs` → O(groups × nodes)

**位置**：`src/ext4/write.rs:576-579`

```rust
576:        gdt[g as usize].used_dirs = nodes
577:            .iter()
578:            .filter(|n| n.kind == Kind::Dir && ino_group(n.ino, ipg) == g)
579:            .count() as u16;
```

**影响**：8 GB 镜像 @4K → 64 组 × 3 万节点 = 2 千万次比较（还好），但 512 MiB @1K → 64 组（E11 实测就是
64 组）；`--size 16G --block-size 1024` → 2048 组 × 节点数 → 数亿次。可在循环外先算一次
`dirs_per_group: Vec<u16>`。

**修法**：进循环前 `let mut used_dirs = vec![0u16; groups]; for n in &nodes { if n.kind == Dir { used_dirs[ino_group(n.ino, ipg)] += 1 } }`，循环里直接取。

---

### R3-M8 `Superblock::parse` 对 `desc_size`/`inode_size` 不做合法值约束

**位置**：`src/ext4/disk.rs:99-100`、`:106`

```rust
 99:            inode_size: u16at(b, 88),
100:            desc_size: u16at(b, 254).max(32),
```

**影响**：`desc_size` 只 `max(32)`，垃圾值（如 1024、4096、奇数）会让 `Ext4::open`（read.rs:25-33）按错误
步长读 GDT，得到完全错乱的 `block_bitmap/inode_table`，随后可能读到文件任意位置并把垃圾当 inode（配合
R3-H4 就是 OOM）。`inode_size` 为 0 时 `inode_location` 里 `byte_off=0` → 永远读每块第 0 个 inode。

**修法**：见 R3-H3 的集中校验（`desc_size ∈ {32,64}`、`inode_size ∈ [128,4096] 且 4 字节对齐`）。

---

### R3-M9 `sparse::parse` / `to_raw` 不校验 `file_hdr_sz` / `chunk_hdr_sz`

**位置**：`src/sparse.rs:58-81`（只校验 major 与 `blk_sz`）、`:83-96`（`off = file_hdr_sz as u64`）、`:96-97`（恒读 12 字节）

**影响**：`file_hdr_sz < 28`（畸形或 CRC 头变体）时 `off` 落在头部中间，把 header 字节当 chunk 解析；
`chunk_hdr_sz < 12` 时 `data_off = off + chunk_hdr_sz` 与 `read_at(f, off, &mut hb[0..12])` 不一致，
要么把 payload 当 chunk 头、要么 `total_sz` 校验全乱。结果是"静默解析出错误 chunk 表"（`sparse info`
输出垃圾），而不是明确报错。libsparse 对这两个字段有 `>= 28` / `>= 12` 的硬校验。

**修法**：
```rust
if (h.file_hdr_sz as usize) < 28 { bail!("bad file header size {}", h.file_hdr_sz); }
if (h.chunk_hdr_sz as usize) < 12 { bail!("bad chunk header size {}", h.chunk_hdr_sz); }
```

---

### R3-M10 `dat::patch` 忽略 bsdiff 行的哈希，`move` 语义与 blockimg 不同

**位置**：`src/dat.rs:126-135`（解析时丢掉 hash token）、`:296-299`（`Move` 当 `Copy` 处理，从 base 读）、
`:317-320`（`stash/free` 空实现）

**影响**：
1. `bsdiff <off> <len> [hash...] <src_rs> <dst_rs>` 的 `hash` 段是 blockimg 用来做完整性校验的，当前直接丢弃
   → 差分打错了不会报错，只会在设备上表现为"镜像哈希不对"。第二轮 N7.5 已提到 `move` 的源语义，
   但"丢哈希"没提。
2. `move` 在 blockimg 里是**从目标（已写）块读**（`move <src> <dst>`，src 指 dst 文件内部的块），
   当前从 base 读。旧版（v1/v2）差分包里 `new` 之后的 `move` 会读错源。README §7 已声明该假设，
   但代码里没有任何警告。

**修法**：`Bsdiff` 结构体加 `hash: Vec<Vec<u8>>` 并在 `patch` 里对 `new_data` 校验（有 hash 时才有意义）；
`Move` 单独分支从 `dst`（`tmp` 文件）读，或至少在遇到 `move` 且 `dst` 已有写入时 `eprintln!("warning: move semantics differ from blockimg")`。

---

### R3-M11 `dat::split` 不做空闲块裁剪，产物比 img2sdat 大 30–50%

**位置**：`src/dat.rs:333-359`

**证据**：`split` 直接 `fs::copy(img, dat)` 然后只发一条 `new 0,blocks`（`:355`）。真 `img2sdat` 会扫
ext4/f2fs 位图，用 `erase` 表示空闲块（产物小 30–50%），并且按 `--version` 输出 `new`/`hash` 等更细的块。

**影响**：功能上 `merge` 能还原（E2E 验证过），但：
- `.new.dat.br` 压不出来（全零块也算数据，brotli 后仍近 1:1，而真工具能到 1:8）；
- 老化机型刷入时按 `new` 逐块写，速度更慢。

**修法**：读 SB 的 `bitmap`（ext4 已有 `Ext4::open`/`read_block`），遍历块位图生成 `erase` 区间；
或在 README §3.5/§7 明写"split 不做 gap 裁剪，产物比 img2sdat 大"。

---

### R3-M12 长符号链接用 `alloc_contig(nblk)`（无折半），`len` 无 u16 上限校验

**位置**：`src/ext4/write.rs:461-484`

```rust
462:                    let nblk = tb.len().div_ceil(bs as usize) as u64;
463:                    let start = alloc_contig(nblk, &mut used, &mut cursor)?;   // 要求整段连续
...
474:                        len: nblk as u16,                                        // 无校验
```

**影响**：文件/目录都走 `alloc_run`（折半 + 多 extent），唯独长符号链接要求一次性连续 `nblk` 块。
1 KiB 块下，一条 1.5 KiB 的目标在碎片化镜像里可能失败（`no space left for 2 blocks`），而同样的空间
分配两个单块完全可行。`nblk as u16` 在 `nblk > 65535` 时静默截断（ext4 链接目标上限 4095 字节，
所以只有 bs=1024 且构造畸形 meta 时才触发）。

**修法**：改用 `alloc_run(nblk.min(8192), ...)` 循环 + 多 extent（与文件一致），并对 `nblk > 32767` 直接
`bail!("symlink target too long")`。

---

### R3-M13 `s_uuid` 由 (volume_name, size) 派生 → 同参数镜像 UUID 相同

**位置**：`src/ext4/write.rs:607-615`

```rust
607:        uuid: opt.uuid.unwrap_or_else(|| {
608:            let mut h = Sha256::new();
609:            h.update(opt.volume_name.as_bytes());
610:            h.update(size.to_le_bytes());
```

**影响**：`ext4 build --label system --size 3G` 跑两次得到**同一个 UUID**。设备上 `by-uuid`/
`/dev/block/by-name` 之外按 UUID 引用的挂载点、以及 `fsck` 的 `s_lastcheck`/`s_uuid` 关联会混淆；
两个不同内容的分区共存时 `blkid` 会报重复 UUID。mke2fs 默认随机。

**修法**：默认用随机（`rand::random::<[u8;16]>()`，`Cargo.toml` 已有 `rand`），并设 RFC4122 v4 版本位
（`u[6] = (u[6] & 0x0f) | 0x40; u[8] = (u[8] & 0x3f) | 0x80;`）；保留 `--uuid` 覆盖。

---

### R3-M14 `super make` 多 group 的 `maximum_size` 各写全额 → 超额

**位置**：`src/superimg.rs:727-738`

```rust
727:    let groups: Vec<Group> = group_names.iter().map(|n| Group {
732:            maximum_size: if n == "default" { opt.group_max.unwrap_or(0) }
735:            else { opt.group_max.unwrap_or(super_size - first_logical * SECTOR) },
```

**影响**：`--partition system_a=...:main --partition vendor_a=...:main2` 时，`main` 与 `main2` 各自
`maximum_size = super 可用空间全额`，总和是实际容量的 2 倍。`lpmake` 只在 group 内有分区时按实际占用累加，
不会给每个 group 全额；设备侧 `lp_allocate` 在动态分区 resize 时才看 group 上限，超额会让
`fastbootd` 的 resize 目标校验通过但实际写失败。

**修法**：按 group 统计实际占用 `sum(align_up(len))` 作为 `maximum_size`（`opt.group_max` 有值时仍优先），
并在构建日志里打印每 group 的 `used/max`。

---

### R3-M15 `payload make` / `super make` 不校验分区重名

**位置**：`src/main.rs:539-548`（payload）、`:397-427`（super）、`src/payload.rs:747-823`、`src/superimg.rs:684-713`

**影响**：`--partition system=a.img --partition system=b.img` 会生成**同一个 `name` 的两条 partition 记录**。
`payload`：manifest 里两条同名 `PartitionUpdate`，`extract` 时后一个覆盖前一个的输出文件（`create_sized`
截断），得到的是 b.img 的内容，a.img 的内容静默消失；`super`：两条同名分区，设备侧 `dm-linear`
按名字建表只能有一个，"幸运"的话是后者生效。

**修法**：构建入口做 `HashSet` 查重，重名即 `bail!("duplicate partition name {}")`。

---

### R3-M16 `payload extract` 用 `new_info.size` 直接 `set_len`，无上限

**位置**：`src/payload.rs:526-529`、`:476-489`（`read_extents` 单 extent 可达 4 GiB）

```rust
526:        let size = p.new_info.as_ref().map(|i| i.size).unwrap_or(0);
529:        let mut out = crate::util::create_sized(&out_path, size)?;
```

**影响**：恶意/损坏 payload 的 `new_info.size = 2^60` → `set_len` 造出 1 EiB 稀疏文件；随后的 op 写入
会把它撑成真实占用，或让后续工具（打包、校验）全部炸掉。`read_extents` 的 `total` 上限是 4 GiB，
但**单个** extent 的 `num_blocks*block` 就能到 4 GiB，一次 `vec![0u8; e.num_blocks*block]` 分配。

**修法**：`if size > (payload file_len * 8).max(16<<30) { bail!("implausible partition size") }`；
`read_extents` 改成按 1 MiB 分块读+写（对 `SOURCE_COPY` 完全可以流式）。

---

### R3-M17 BSDF2（Android 10+ 常用）仍不支持（第二轮 N6.1 未修）

**位置**：`src/payload.rs:646-649`

```rust
647:    if patch.len() >= 4 && &patch[0..4] == b"BSDF2" {
648:        bail!("BSDF2 (compressed bsdiff) patches are not supported");
```

**影响**：Android 10+ 的 `SOURCE_BSDIFF` blob 常见 `BSDF2` 头（第三段按压缩器 id 选 bz2/brotli/raw，
`vendor/payload-dumper-go/internal/bspatch` 两者都支持）。README §3.4 声称"提取支持 SOURCE_BSDIFF"，
对这类包会直接失败。当前至少是明确报错（不是静默错数据），故定为 Medium 而非 High。

**修法**：按 bsdiff 4.3 + AOSP 扩展解析 `BSDF2`：
```rust
// magic(8) | ctrl_len | diff_len | new_size | bz2_len? | 第三段压缩器 id
// 若 patch[0..4]=="BSDF2": 读 patch[4] 的压缩器 id（0=bz2,1=brotli,2=raw）
```
优先只做 `id=0 (bz2)` 与 `id=2 (raw)`（覆盖绝大多数），brotli 用已有 `brotli` crate。

---

### R3-M18 `--key` 只接受 PKCS#8 **PEM**，AOSP `testkey.pk8` 无法直接用

**位置**：`src/payload.rs:884-891`

```rust
885:        let pem = fs::read_to_string(key_path)...;
887:        let key = RsaPrivateKey::from_pkcs8_pem(&pem).context("parse PKCS#8 RSA key")?;
```

**影响**：AOSP 签名材料是 `testkey.pk8`（**DER** 编码的 PKCS#8，无 PEM armor），
`fs::read_to_string` 会因非法 UTF-8 失败或 `from_pkcs8_pem` 报错。用户必须先用
`openssl pkcs8 -inform DER -in testkey.pk8 -out key.pem` 转换，README 未提。

**修法**：先按字节读，尝试 DER → PEM 两种：
```rust
let raw = fs::read(key_path)?;
let key = RsaPrivateKey::from_pkcs8_der(&raw)
    .or_else(|_| std::str::from_utf8(&raw).map_err(...).and_then(|s| RsaPrivateKey::from_pkcs8_pem(s)))...
```

---

## 4. Low

### R3-L1 编译告警（唯一一条）

`src/ext4/write.rs:11` `use std::fs::{self, File};` 的 `self` 未使用（`fs::` 已不再出现）。
`cargo build` 输出 `warning: unused import: self`。第二轮报告 §0 声称"0 warning"，已失效。
**修法**：`use std::fs::File;`（或 `fs::rename` 之类真的用一下）。

### R3-L2 死代码清单（`pub` 所以无 warning）

| 位置 | 项 |
|---|---|
| `src/ext4/disk.rs:694` | `checksum_u32` 恒返回 0（`metadata_csum` 未实现）。**读到带该特性的镜像会写出错误校验和**，建议读 `feature_incompat & 0x400` 时显式 `bail!` |
| `src/ext4/disk.rs:542` | `build_dir_block`（与 `write.rs::pack_dir_blocks/render_block` 重复实现） |
| `src/ext4/read.rs:572` | `name_hash`（注释称"builder 用于目录排序"，实际未用） |
| `src/superimg.rs:317` | `slot_label`（直接返回 `p.name`） |
| `src/dat.rs:427` | `patch_stream_offset`（读一行后恒返回 0） |
| `src/sparse.rs:367` | `resparsify`（且 `NamedTempFile` 落 `%TEMP%`，多 GB 镜像撑爆 C:） |
| `src/sparse.rs:411` | `read_raw`（只被测试用） |
| `src/sparse.rs:418` | `seek_to_end` |
| `src/util.rs:16` | `align_down` |
| `src/util.rs:68` | `read_vec_at` |
| `src/perm.rs:513` | `by_path` |
| `src/boot.rs:93` | `BootImage.bootconfig` 在 boot 路径恒空（vendor 用）—— 建议注释说明 |

### R3-L3 `align_up` / `parse_size` 溢出（实测 E5，第二轮 N16 未修）

`src/util.rs:13` `(v + a - 1) / a * a`、`src/util.rs:110` `Ok(v * mult)`。
实测 `--size 300000000000G` → **panic at util.rs:110:8 attempt to multiply with overflow**（exit 101）。
**修法**：`v.checked_mul(mult).ok_or_else(|| anyhow!("size overflow"))?`；`align_up` 用
`v.checked_add(a - 1).map(|x| x / a * a)`，失败返回 `v`（或 `bail!`）。

### R3-L4 `extract_tree` 无 visited 集合 → 目录环无限展开

`src/ext4/read.rs:372-457`：`queue.pop()` 的 BFS/DFS 只按 `(ino, vpath)` 入队，不记 `visited ino`。
构造一个目录项指回祖先（`/a/loop -> /a`）的镜像，会无限入队 + 无限 `create_dir_all`，最终把
路径长度推到 Windows 的 `MAX_PATH` 报错或把 inode 表读爆。**修法**：`HashSet<u32>` 记录已展开目录，
重复即 `bail!("directory cycle at inode {}")`（真 ext4 不允许硬链接目录，报错是对的）。

### R3-L5 `tree_rec` 递归深度无上限

`src/ext4/read.rs:506-554`。深目录（>1000 层，构造镜像很容易）→ 栈溢出 abort（不是 `Err`）。
**修法**：加 `depth > 64` 的 `bail!`，或改迭代 + 显式栈。

### R3-L6 解包不检查输出目录里已存在的 symlink / junction（第二轮 N13 未修）

`src/ext4/read.rs:396-421`、`:389-393`。若 `<out>/system` 事先被做成指向 `C:\Windows` 的 junction，
后续 `create_dir_all(out/system/bin)` 与所有文件写入都会落到树外。**修法**：每个中间目录 `create_dir_all`
之前 `fs::symlink_metadata` 检查 `is_symlink()`（Windows 上 junction 需 `FILE_ATTRIBUTE_REPARSE_POINT`，
可用 `std::os::windows::fs::MetadataExt::file_attributes()`），命中即 `bail!`。

### R3-L7 `meta.txt` 的 `link` 字段未转义 → 可注入行

`src/perm.rs:501`（`save` 直接 `e.link.clone().unwrap_or_default()` 拼进 TSV）、`src/ext4/read.rs:404`
（`link` 来自镜像里的任意字节，经 `cstr` 成 `String`）。符号链接目标里含 `\n` 时，`MetaDb::parse`
会把后面的内容当成一条新记录 → `meta.txt` 注入（后续 `ext4 build --meta` 会据此创建任意路径/权限）。
**修法**：`save` 时把 `\t`/`\n`/`\r` 转义为 `\t`/`\n`/`\r` 字面量并在 `parse` 里反转义；或直接
`bail!("symlink target contains control characters")`（ext4 里本就罕见）。

### R3-L8 `from_dir` 不记录 ino、不拒绝 junction（第二轮 N14 未修）

`src/perm.rs:354-446`：`ino: None` 是常量 → 从 Windows 树 `perm export` → `ext4 build` 永远丢硬链接
（只在"从 ext4 解包"这条路上保留）。且 `ft.is_symlink()` 对 NTFS junction 返回 **false**、`is_dir()` 返回
true → `stack.push(p)` 会递归进 junction，可能无限循环或扫到 `C:\` 全盘。
**修法**：(a) `ino` 用 `(st_dev, st_ino)`（Windows 用 `GetFileInformationByHandle` 的
`nFileIndexHigh/Low`，`std::os::windows::fs::MetadataExt::file_index()` 在 1.63+ 可用）；
(b) `kind == Dir` 且 `metadata.file_attributes() & 0x400 != 0`（REPARSE_POINT）时按 Link 处理或跳过。

### R3-L9 `ext4 extract` 对大小写不敏感冲突无保护（第二轮 N13 未修）

`src/ext4/read.rs:389-421`：ext4 里 `Foo` 与 `foo` 是两个文件，NTFS 上是同一个 → 后写的覆盖前写的，
且**没有任何提示**（`super unpack` 有 `seen` 集合，ext4 没有）。**修法**：与 `superimg` 一致，维护
`HashSet<lowercased path>`，冲突时 `bail!` 或 `eprintln!("warning: case-insensitive collision {}")`
并按 `~1` 后缀改名。

### R3-L10 `--metadata-slots 0` 不校验

`src/superimg.rs:774-779`：`for slot in 0..0` 不写任何 metadata，但 `create_sized` 已把镜像建好、
`geometry` 也写了 → 产物看似正常，`super dump` 必然失败。**修法**：`if opt.metadata_slots == 0 || opt.metadata_slots > 8 { bail!() }`。

### R3-L11 `payload` 不校验 metadata 签名 / 不检查 `partial_update`

`src/payload.rs:359-405` 读了 `metadata_signature` 但从不校验（README §7 已声明"签名只覆盖 metadata manifest"，
但"生成了却不校验"是两件事）。**修法**：加 `--verify-signature key.pem` 或至少 `payload info` 打印
`signature: N bytes (not verified)`。

### R3-L12 Python 侧的类型/校验不一致

| 位置 | 问题 |
|---|---|
| `python/pyaimg/types.py:374-391` | `MetaEntry.from_tsv` 检查 `len(f) < 8` 但读 `f[8]` → 8 字段的行 `IndexError`；Rust 侧容忍（`f.get(8).unwrap_or(0)`） |
| `python/pyaimg/types.py:529-553` | `SparseInfo.from_file` 不校验 `len(ch)==12`（`struct.unpack` 抛 `struct.error`）、不校验 `total_sz` |
| `python/pyaimg/types.py:176-178` | `PayloadExtent.size` 硬编码 4096，忽略 manifest 的 `block_size` |
| `python/pyaimg/types.py:136-144` | `SuperInfo.size_of` 直接索引进 `extents`，畸形元数据 → `IndexError`（Rust 用 `.get()`） |
| `python/pyaimg/perm.py:52` | `int(f[3], 8)` 不认 `0o755` 前缀（Rust 侧 `trim_start_matches("0o")` 认） |
| `python/pyaimg/perm.py:102` | `-d` 选择器要求 `len(pat) == 2`，Rust 侧接受 `-d/path` |
| `python/pyaimg/perm.py:166-167` | `MetaDb.get` 线性扫描（同 R3-H8，Python 侧小数据量无碍） |

### R3-L13 `dat::merge` 对 `zero/erase` 真的写 0（不 seek 跳过）

`src/dat.rs:195-202`：目标文件是 `create_sized` 出来的全零文件，再逐块写 0 是纯冗余 I/O。
4 GB 镜像里若 30% 是 `erase`，就白写 1.2 GB。**修法**：`Cmd::Zero/Erase` 直接 `dst.seek(...)` 跳过
（文件已是零；若要支持"覆盖已有前缀"再写作）。

### R3-L14 `payload::build` 二次读整个输出算 FILE_HASH

`src/payload.rs:852-861`：写完再 `File::open(out)` 全量读一遍算 SHA256（3 GB payload → 多 3 GB I/O）。
**修法**：`io::copy(&mut tf, &mut f)` 时用 `Sha256` 包一层读写器，边写边算。

### R3-L15 `boot pack` 静默丢弃 AVB footer

`src/boot.rs:645-656`：`avb_footer: None` 恒成立；`unpack` 时会裁剪 AVB 并把 `avb` 字段留在 `boot.json`
（`:587-592`），`pack` 时既不重建也不提示。用户会以为"改完重新打包还能过 AVB"。README §7 已声明
"AVB 只做 footer 识别与裁剪，不重签 vbmeta"，但 `boot.json` 里留着 `avb` 字段会误导。
**修法**：`pack` 时若 `desc.avb.is_some()` 打印 `eprintln!("warning: AVB footer/vbmeta dropped; re-sign with avbtool")`。

### R3-L16 `write.rs` 的 `ipg` 下调用向下取整

`src/ext4/write.rs:190`：`itable_blocks = ((ipg * inode_size) / bs).max(1)`，而上面 `:174` 用的是
`div_ceil`。当前所有参数组合（`ipg=8192/2048`、`inode_size=256`、`bs∈{1024..65536}`）都能整除，
所以是**潜在**问题：一旦将来支持 `inode_size=160/320` 或别的 `ipg`，最后一组的 inode table 会少一块，
最后一个 inode 落到下一块（e2fsck `inode table extends past end`）。**修法**：统一 `div_ceil`。

### R3-L17 `superimg::open_maybe_sparse` 把 sparse 展开到 `%TEMP%`

`src/superimg.rs:125-134`：`NamedTempFile::new()` → `%TEMP%`（默认 C:）。3–8 GB 的 super.simg 会把
系统盘撑爆（本机 C: 尤其紧张）。与 `sparse::resparsify` 同一类问题。**修法**：临时文件放到输出目录旁边
（`NamedTempFile::new_in(out_parent)`），或加 `--work-dir`；并在展开前检查目标盘剩余空间。

### R3-L18 `payload::extract` 每个 op 反复分配 blob 缓冲

`src/payload.rs:447-460`：`read_blob` 每次 `vec![0u8; len]`；2 MiB chunk × 数千 op → 数千次 2 MiB 分配/释放
（`read_extents` 同）。**修法**：在 `extract` 里持有一个 `Vec<u8>` 复用缓冲，`read_blob` 增加 `&mut buf` 参数。

### R3-L19 `super unpack` 重复读 geometry

`src/superimg.rs:415` `read_geometry(&mut f)?;` 的返回值被丢弃，紧接着 `parse_slot`（`:417`）内部
`geometry_base` + `read_geometry_at` 又读一遍（含两次 SHA256）。无害但多余，且 `metadata_offset` 用的是
`GEOMETRY_SIZE` 而非 `base`（见下一项）。

### R3-L20 `metadata_offset` 的 `base_off` 与 `geometry_base()` 探测结果可能不一致

`src/superimg.rs:185-192` 的 `base_off` 是调用方传的常量；`parse_slot` 传 `base`（探测值，见 `:195`），
而 `build` 里（`:775`/`:777`）硬编码 `GEOMETRY_SIZE`。若将来支持"geometry at 0"的 legacy 布局，
**写**路径会与**读**路径错开 4096。**修法**：`build` 也用 `geometry_base`（或把 `base_off` 收进 `Geometry`）。

### R3-L21 README 与现状不符

| 位置 | 声明 | 现状 |
|---|---|---|
| §8 | `unit: 18 passed`、`doc-tests: 0`、`编译器警告: 0` | 实测 **21 unit + 2 e2e**、**1 warning**（R3-L1）；验证日期 2026-11-10 早于 11-15 的大改 |
| §3.6 | "构建器实现的是 ext4 的一个可靠子集：**4 KiB 块**" | CLI 允许 1024–65536，实测都能构建（E16） |
| §8.1 末行 | "payload 缺 `payload_properties.txt` …… 生成 `.properties`" | 文件名不是 `payload_properties.txt`（R3-M2），工具链不认 |
| §3.4 | "提取支持 … `SOURCE_BSDIFF`（内置 bspatch）" | BSDF2 变体不支持（R3-M17） |
| §7 | "`stash`/`free` 可忽略（源始终从只读 base 读）" | `move` 的 blockimg 语义是从目标读（R3-M10） |
| §6.1 | Windows 交叉编译产物"功能与 MSVC 构建一致" | 未在本轮验证（无 Docker/WSL 可用） |

---

## 5. 第二轮（N1–N17）复核结果

| 编号 | 第二轮结论 | 本轮复核 |
|---|---|---|
| N1 1K 块 8 MiB 文件 | 已修 | **确认已修**（E11：构建+解包 MD5 一致） |
| N2 全量进内存 | 已修 | **部分**：文件数据已流式；`extra_blocks`（每 inode 4 KiB xattr + 目录块）仍全量驻留 → R3-H9（实测 107.7 MiB / 20k inode） |
| N3 meta_end 少 4096 | 已修 | **确认已修**（E12：`first_logical_sector=536`） |
| N4 双槽剥后缀 | 已修 | **确认已修**（E13） |
| N5 sparse 分区 | 已修（super make 侧） | **部分**：`ext4` 仍不识别 sparse → R3-M4（实测 `bad ext4 magic 0xff2`） |
| N6.1 BSDF2 | 未修 | **仍未修** → R3-M17 |
| N6.2 properties | 已"生成" | **仍不兼容**：文件名错 → R3-M2 |
| N6.3 末块补齐 | 已修 | 确认（`op.dst_length = pad_to`，`extract` 末尾 `set_len(size)`） |
| N7 dat split/patch | 未改 | **仍未改** → R3-M10/M11/L13 |
| N8 ibody xattr | 未改 | **仍未改**：每个 inode 一个 4 KiB 外部块（R3-H9 的内存代价即由此放大 4 倍） |
| N9 extract 写 meta.txt | 已修 | **只压了 warning**，`meta.txt` 仍被打进镜像 → R3-M3（E18） |
| N10 v3/v4 地址字段 | 已修 | 确认（`:183-189` 置 0） |
| N11 Cargo.lock 同步 | 已修 | 确认（本轮 `cargo build` 未改 lock） |
| N12 死代码 | 未改 | **仍未改** → R3-L2 |
| N13 safe_component 边界 | 未改 | **仍未改** → R3-L6/L9 |
| N14 硬链接 | 未改 | **仍未改** → R3-L8 |
| N15 `--verify` 带值 | 已修 | 确认（`main.rs:178` `num_args=0..=1`） |
| N16 性能/溢出 | 部分 | `tree_rec` 已改计数器；`align_up`/`parse_size` 溢出**仍在**（E5 实测 panic）→ R3-L3；新的 O(n²) 在 `MetaDb::insert` → R3-H8 |
| N17 README §8 过期 | 未改 | **仍未改** → R3-L21 |

---

## 6. 建议修复顺序

| 顺序 | 项 | 理由 |
|---|---|---|
| 1 | R3-H1 / H2 / H3 | 三处（实为四处）panic，改动都只是"加校验/换安全切片"，各 3–8 行 |
| 2 | R3-H7 | 删一行 `sync_all()`，解包从 25 min 回到秒级 |
| 3 | R3-H8 | 去掉 `insert` 或加索引，10 行内 |
| 4 | R3-H10 | auto-size 纳入 inode 数量 + 补 `--inodes`，否则"解包→回包"最容易踩 |
| 5 | R3-H9 | 去掉 `extra_blocks` 缓冲（分配即写），把峰值从 O(inode×4KiB) 降到常数 |
| 6 | R3-H4 / H5 / H6 | 三条"外部输入 → 大分配/大循环"的统一上限 |
| 7 | R3-M1 / M2 / M3 / M4 | 行为正确性 + 与工具链对齐，改动都很局部 |
| 8 | R3-M12 / M13 / M14 / M15 / M16 | 构建端正确性 |
| 9 | 其余 Medium / Low | 清理、文档、Python 侧 |

---

## 7. 未验证 / 覆盖缺口

- **未跑 e2fsck**：本机（Windows、无 WSL、内存 8 GB/可用数百 MB）无法安装 e2fsprogs，第二轮在
  aws Docker 里做过的 `e2fsck -fn` 本轮**未复跑**。E11/E16 的镜像"能自读"不能替代 e2fsck；
  1 KiB 块 + 64 组（E11）与 64 KiB 块（E16）这两条尤其需要 `e2fsck -fn` + `dumpe2fs` 复核。
- **R3-H4 / H5 / H6 未实跑**：都是"会让本机 OOM 或长时间挂死"的路径，只做静态确认（代码路径明确）。
  建议在容器里用 `ulimit -v` 限制后复现（例如 `ulimit -v 2000000` 后喂 `tables_size=4GiB` 的 super）。
- **真实样本缺失**：手头没有 BSDF2 的 delta payload、没有 OEM 的 `super.img`（多 group/多 slot）、
  没有带 `metadata_csum`/`inline_data`/htree 的原厂 ext4，因此 R3-M17、htree 读取、`checksum_u32` 的
  实际影响都只是静态判断。
- **`python/` 未跑**：本机未装 pytest/mypy（`python/tests/test_api.py` 的 10 个用例未执行），
  R3-L12 全部为通读结论。
- **Windows 交叉编译**（README §6.1）未验证（无 Docker）。
- **测试覆盖缺口**（新增建议）：`tests/e2e.rs` 只有 2 个 happy-path 用例，21 个单测里**没有任何**
  "畸形输入必须返回 `Err` 而不是 panic"的断言。建议加一组 fuzz-ish 用例：
  `boot::{truncated_v3, v3_len_1584}`、`payload::{bad_varint_len, tables_size_huge}`、
  `ext4::{bpg_zero, ipg_zero, hole_bomb}`、`superimg::{entry_size_zero, tables_size_huge}`，
  每个都用 `std::panic::catch_unwind` 或 `assert!(result.is_err())` 固化，防止回归。

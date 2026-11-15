# AndroidImg_Rust (`aimg`)

Windows 上没有 POSIX uid/gid、没有 mode bits、没有 SELinux label，Linux 那套"等级森严"的权限模型在 NTFS 上根本落不了地。
这个项目把整条 Android 镜像工具链用 Rust 重写了一遍，并且**把权限/上下文从文件系统里抽出来，改成纯文本配置**：
镜像里该是什么 uid/gid/mode/`security.selinux`，由 `fs_config.txt` + `file_contexts` + `meta.txt` 决定，
和宿主机 NTFS ACL 完全解耦。

所有实现都在用户态完成：不 mount、不 loop、不要 root、不依赖 WSL。

---

## 1. 目录结构

```
AndroidImg_Rust/
├── Cargo.toml
├── src/
│   ├── main.rs            # clap CLI（aimg 子命令）
│   ├── lib.rs
│   ├── util.rs            # 对齐/随机 IO/大小解析/哈希
│   ├── sparse.rs          # Android sparse image（simg2img/img2simg）
│   ├── superimg.rs        # super.img LP metadata（lpdump/lpunpack/lpmake）
│   ├── boot.rs            # boot/recovery/vendor_boot v0-v4 + AVB footer
│   ├── payload.rs         # payload.bin（protobuf/ops/bsdiff/回包/签名）
│   ├── dat.rs             # .dat / transfer.list / patch.dat / brotli
│   ├── perm.rs            # fs_config + file_contexts + meta.txt
│   └── ext4/
│       ├── disk.rs        # 磁盘结构：superblock / GDT / inode / extent / xattr
│       ├── read.rs        # 用户态只读解析 + 解包到目录
│       └── write.rs       # 从目录构建 ext4（extents、xattr、bitmap）
├── tests/e2e.rs           # 端到端流水线测试
├── configs/               # 样例文本配置
├── python/                # pyaimg：类型化 Python API + .pyi 存根
├── scripts/               # Docker 构建 / e2fsck 验证脚本
├── docs/                  # 上游工具调研
└── vendor/                # 上游参考源码（只读，不参与编译）
```

`vendor/` 是 17 个上游仓库的浅克隆，用来对照格式与语义：
`payload-dumper-go`、`payload_dumper`、`Android_boot_image_editor`、`sdat2img`、`img2sdat`、
`imgpatchtools`、`android-simg2img`、`Payload_Repack_Tool`、`Android-Image-Kitchen`、
`lpunpack_and_lpmake`，以及 AOSP 的 `system/extras`(partition_tools/ext4_utils)、
`system/core`(libsparse)、`system/tools/mkbootimg`、`system/update_engine`、
`external/e2fsprogs`、`external/avb`。

---

## 2. 权限模型（本项目与 Linux 工具最大的区别）

### 2.1 为什么不用 NTFS ACL

NTFS ACL 是"谁可以访问这个文件"的访问控制，语义和 POSIX mode 并不对应，更不能表达 SELinux label。
Windows 上复制/解压还会丢掉这些信息。因此本项目**只把文本当权威来源**：

| 文件 | 作用 | 格式 |
|---|---|---|
| `fs_config.txt` | 路径 → uid / gid / mode / capabilities | AOSP 原生格式 |
| `file_contexts` | 路径正则 → SELinux context | AOSP 原生格式 |
| `meta.txt` | 解包出来的整棵树最终解析结果（TSV，可 diff/手改） | 本项目定义 |

### 2.2 `fs_config.txt`

```
# path  uid gid mode  [capabilities=0x...]
/system 0 0 0755
/system/bin 0 2000 0755
/system/bin/sh 0 2000 0755
/system/bin/su 0 0 04755 capabilities=0x40
```

查找规则：先精确匹配；没有再向上找最近的父目录规则（继承 uid/gid，mode 用默认值：目录 0755 / 文件 0644 / 链接 0777）。

### 2.3 `file_contexts`

```
/system(/.*)?        u:object_r:system_file:s0
/system/bin/sh       u:object_r:shell_exec:s0
/system/bin/.*       u:object_r:system_bin:s0
```

匹配规则模拟 `selabel_lookup` 的"最具体者胜"：取字面前缀最长的规则；长度相同取后定义的。

### 2.4 `meta.txt`（TSV）

```
# aimg-meta v1
# kind	path	uid	gid	mode	context	caps	size	mtime	link_target	ino
D	/system	0	0	0755	u:object_r:system_file:s0	0x0	0	1700000000	
F	/system/bin/sh	0	2000	0755	u:object_r:shell_exec:s0	0x0	17	1700000000	
L	/system/bin/ls	0	2000	0777	u:object_r:system_file:s0	0x0	0	1700000000	/system/bin/toybox
```

- 解包（`ext4 extract`）时从镜像里读出真实的 uid/gid/mode/xattr（`security.selinux`）写入 `meta.txt`；
- 回包（`ext4 build --meta`）时把它逐条写进 inode 与 xattr block；
- 想在 Windows 上改权限，直接编辑这个文本即可，随后 `perm apply` 只会还原只读位和 mtime（因为 NTFS 只能表达这些），
  真正的 uid/gid/SELinux 在下次 `ext4 build` 时生效。

---

## 3. 命令参考

### 3.1 sparse（对应 `simg2img` / `img2simg`）

```bash
aimg sparse info system.img
aimg sparse to-raw system.img_sparsechunk.0 system.img_sparsechunk.1 --out system.raw   # 多卷合并
aimg sparse from-raw system.raw --out system.simg --block-size 4096
```

### 3.2 super（对应 `lpdump` / `lpunpack` / `lpmake`）

```bash
aimg super dump super.img --slot 0
aimg super unpack super.img --out out/ --slot 0 [--keep-suffix] [--sparse]
# --slot 同时选择 metadata 副本和 A/B 后缀：--slot 0 只导出无后缀与 *_a，--slot 1 只导出无后缀与 *_b
aimg super make --out super_new.img \
  --partition system_a=system.img:main \
  --partition vendor_a=vendor.img:main \
  --size 8G --metadata-max-size 65536 --metadata-slots 2 --minor-version 2
```

要点：`--metadata-slots`、group 名、分区顺序、`--size` 必须和原厂 `lpdump` 输出一致，否则设备侧 `dm-linear` 映射失败。

### 3.3 boot（对应 `unpack_bootimg.py` / `mkbootimg.py` / AIK）

```bash
aimg boot info boot.img
aimg boot unpack boot.img --out boot_dir/     # 产出 boot.json + kernel/ramdisk/dtb/...
# 改完文件
aimg boot pack boot_dir/ --out new-boot.img
```

覆盖 header v0–v4、`vendor_boot` v3/v4（含 ramdisk table + bootconfig）、AVB footer 识别与裁剪。

### 3.4 payload（对应 `payload-dumper-go` / `delta_generator`）

```bash
aimg payload info payload.bin
aimg payload extract payload.bin --out out/ [-p boot,system] [--old base_dir/]
aimg payload make --out payload_new.bin --partition boot=boot.img --partition system=system.img \
  [--compress] [--key key.pem] [--chunk-size 2097152]
```

- 提取支持：`REPLACE` / `REPLACE_BZ` / `REPLACE_XZ` / `ZERO` / `DISCARD` / `SOURCE_COPY` / `SOURCE_BSDIFF`（内置 bspatch）；
- 增量包必须提供 `--old`（base 镜像目录，文件名 `<partition>.img`）；
- 仅支持 `REPLACE`/`REPLACE_BZ`/`REPLACE_XZ`/`ZERO`/`DISCARD`/`SOURCE_COPY`/`SOURCE_BSDIFF`；
  PUFFDIFF / ZUCCHINI / LZ4DIFF 会明确报 `unsupported type`，不会静默产出错镜像；
- `--verify` 校验每个 op 的存储 blob 哈希，并在分区写完后比对 `new_info.hash`；
- 回包生成 minor_version 0 的 full payload；带 `--key` 时对 manifest 做 RSA PKCS#1 v1.5 SHA256 签名。

### 3.5 dat（对应 `sdat2img` / `img2sdat` / `imgpatchtools`）

```bash
aimg dat info system.transfer.list
aimg dat br system.new.dat.br --out system.new.dat
aimg dat merge system.transfer.list system.new.dat --out system.img
aimg dat split system.img --out out/ --prefix system --version 4
aimg dat patch base.img system.transfer.list system.new.dat system.patch.dat --out new.img
```

### 3.6 ext4（对应 `debugfs` / `e2fsdroid` / `make_ext4fs`）

```bash
aimg ext4 info system.img
aimg ext4 ls system.img --limit 200
aimg ext4 extract system.img --out system_root/ [--meta custom.txt] [--file-contexts fc]
aimg ext4 build --dir system_root/ --out system.img --size 2G \
  [--meta meta.txt] [--fs-config fs_config.txt] [--file-contexts file_contexts] \
  [--block-size 4096] [--label system]
```

构建器实现的是 ext4 的一个可靠子集：4 KiB 块、256 B inode、`extents` + `filetype` + `sparse_super`，
无 journal、无 `metadata_csum`、无 flex_bg、线性目录（不做 htree），SELinux context 写入外部 xattr block。
`e2fsck -fn` 可过（见第 6 节验证流程）。

### 3.7 perm（文本权限体系）

```bash
aimg perm export --dir root/ --out meta.txt --fs-config fs_config.txt --file-contexts file_contexts
aimg perm apply  --meta meta.txt --dir root/      # 只还原只读位与 mtime
aimg perm show   --meta meta.txt /system/bin/sh
```

---

## 4. 典型工作流（Windows 上完整闭环）

```bash
# 0) 从 OTA 里拿到 payload.bin
aimg payload extract payload.bin --out parts/

# 1) 动态分区拆包
aimg super unpack super.img --out parts/ --slot 0

# 2) sparse -> raw -> ext4 解包
aimg sparse to-raw parts/system.img --out parts/system.raw
aimg ext4 extract parts/system.raw --out system_root/

# 3) 在 Windows 上直接改文件，改权限就编辑 meta.txt
#    例如把 /system/bin/foo 改成 0:2000 0755 shell_exec

# 4) 回包
aimg ext4 build --dir system_root/ --out parts/system.raw --meta system_root/meta.txt --size 3G

# 5) 转 sparse、塞回 super、生成 payload
aimg sparse from-raw parts/system.raw --out parts/system.img
aimg super make --out super_new.img --partition system_a=parts/system.img:main --size 8G
aimg payload make --out payload_new.bin --partition system=parts/system.raw
```

---

## 5. 与上游源码的对应关系

| 模块 | 重写自 | 关键差异 |
|---|---|---|
| `sparse.rs` | AOSP `libsparse` + `android-simg2img` | 纯 Rust，支持多卷合并与 FILL/DONT_CARE/CRC32 |
| `superimg.rs` | AOSP `partition_tools` (liblp) | 纯 Rust 解析/生成 metadata，SHA256 校验，slot 与备份副本 |
| `boot.rs` | AOSP `mkbootimg` + `Android_boot_image_editor` | 统一 v0-v4 + vendor_boot，JSON 描述符回灌 |
| `payload.rs` | `payload-dumper-go` / `update_engine` | 手写 protobuf 编解码，不依赖 protoc；内置 bspatch |
| `dat.rs` | `sdat2img` / `img2sdat` / `imgpatchtools` | 合并/拆分/差分三合一，brotli 内置 |
| `ext4/*` | `e2fsprogs`(debugfs) + `e2fsdroid` | 纯 Rust 读写，权限来自文本而非宿主文件系统 |

---

## 6. 构建与验证（不在宿主机装环境）

本机只做编辑；编译与验证全部在远程 Docker 里完成：

```bash
# 本机：打包源码
tar -czf aimg-src.tgz Cargo.toml Cargo.lock src tests configs
scp aimg-src.tgz aws:~/aimg-src.tgz
ssh aws 'rm -rf ~/AndroidImg_Rust && mkdir -p ~/AndroidImg_Rust && tar -xzf ~/aimg-src.tgz -C ~/AndroidImg_Rust'

# 远程：编译 + 测试
ssh aws 'docker run --rm -v /root/AndroidImg_Rust:/w -w /w rust:1-slim \
  bash -c "cargo test"'

# 远程：用真正的 e2fsprogs 校验生成的镜像
ssh aws 'docker run --rm -v /root/AndroidImg_Rust:/w debian:stable-slim \
  bash -c "apt-get update -qq && apt-get install -y -qq e2fsprogs >/dev/null && e2fsck -fn /w/out/test.img"'
```

`tests/e2e.rs` 会跑完整链路：树 → meta → ext4 → sparse → super → payload → dat，再逐级回读比对。

### 6.1 Windows 二进制交叉编译（推荐）

不必在 Windows 本机跑 `cargo build`：Linux 的 Docker 里可以直接交叉编译 `x86_64-pc-windows-gnu`。

```bash
scripts/build_windows_in_docker.sh .
# 产物：target/x86_64-pc-windows-gnu/release/aimg.exe
```

容器内做的事情：`apt install gcc-mingw-w64-x86-64`、`rustup target add x86_64-pc-windows-gnu`，
并把 `CC_/CXX_/AR_x86_64_pc_windows_gnu` 指向 mingw 工具（`bzip2-sys` 这类 C 依赖也能过）。

产物只依赖系统 DLL（KERNEL32 / msvcrt / ntdll / bcryptprimitives / api-ms-win-core-synch），
不依赖 `libgcc_s_seh-1.dll` / `libwinpthread-1.dll`，可直接在干净 Windows 上跑；功能与 MSVC 构建一致
（同一镜像上 `ext4 info` 输出逐字节相同）。

验证策略：逻辑用 Linux target 的 `cargo test`，Windows 侧交叉编译 + 本机冒烟（`aimg.exe --version` /
`ext4 info`）；需要跑 Windows 单测时可在容器里装 Wine 执行 `cargo test --target x86_64-pc-windows-gnu`。

---

## 7. 已知限制

- ext4 构建器不写 journal，不启用 `metadata_csum`；对原厂镜像做**原地修改**（in-place patch）尚未实现，当前是"解包 → 重建"模式。
- ext4 读取支持 extents / 线性目录 / 外部 xattr；htree 目录按叶子扫描（可读），不重建 htree。
- payload 回包只生成 full payload，不做增量 diff 生成；签名只覆盖 metadata manifest。
- AVB 只做 footer 识别与裁剪，不重签 `vbmeta`（需要私钥 + `avbtool` 等价实现，后续补）。
- `.dat` 的 `bsdiff` 行遵循 blockimg 语法：`bsdiff <patch_off> <patch_len> [hash...] <src_rs> <dst_rs>`，
  补丁数据是 `patch.dat` 里按 `<off>/<len>` 定位的整段字节（没有长度前缀）；`move` 等同 `copy`，
  `stash`/`free` 在文件级 patch 中可忽略（源数据始终从只读 base 读取）。

---

## 8. 实测验证结果（2026-11-10，aws Docker）

编译与测试全部在 `rust:1-slim` 容器内完成（宿主机零安装、零 WSL）：

```
cargo build --release      Finished in 38.9s
cargo test
  unit:        18 passed
  tests/e2e:    2 passed   (树→meta→ext4→sparse→super→payload→dat 全链路)
  doc-tests:    0
编译器警告: 0

python -m pytest -q        (python:3-slim, AIMG_BIN 指向 release 二进制)
  10 passed in 0.24s

python -m mypy pyaimg tests --ignore-missing-imports
  Success: no issues found in 12 source files
```

ext4 产物交给真正的 `e2fsprogs 1.47.2` 校验（`debian:stable-slim` 容器）：

```
e2fsck -fn /v/system.img
Pass 1..5 ... clean
e2fsck_exit=0
debugfs -R "ea_list /system/bin/blob"
  security.selinux (26) = "u:object_r:system_file:s0\000"
debugfs -R "stat /system/bin/blob_link"
  Type: symlink   Mode: 0777   User: 0  Group: 2000  Blockcount: 8
```

即：uid/gid、mode、SELinux context、符号链接、extent 布局全部被 Linux 侧工具认可。

### 8.1 写 ext4 时踩到的真实坑（已在代码里修掉）

| 现象 | 原因 | 修法 |
|---|---|---|
| `e2fsck: Group descriptor 0 marked uninitialized without feature set` | 无 `gdt_csum` 特性时 `bg_itable_unused != 0` 即报错 | 无该特性时强制写 0 |
| `Block bitmap differences: +(2--3)` | group 0 的 block/inode bitmap 块自己没标记占用 | 布局阶段一并 mark |
| `Extended attribute ... has a hash (0) which is invalid` | xattr block 的 `h_hash` 与每个 entry 的 `e_hash` 必须按 rotate-xor 计算，值按 4 字节补齐参与哈希 | 实现 `ext2fs_ext_attr_hash_entry` / `ext2fs_ext_attr_block_rehash` |
| `system.selinux` 而不是 `security.selinux` | ext4 xattr name index：security=6、system=7（4=trusted、1=user） | 修正常量与解析映射 |
| `i_blocks is 8, should be 16` | xattr 块也要计入 `i_blocks` | 分配 xattr 块后 `i_blocks += bs/512` |
| boot v3/v4 解析出 `page_size=0` | v3 头 24..40 是 reserved，页大小固定 4096 | 版本 >=3 时直接取 4096 |
| `meta.txt` 里全是 `u:object_r:unlabeled:s0` | 现代 ext4 把 `security.selinux` 放在 inode **内联 xattr 区**（magic 在 `128 + i_extra_isize`），只读外部 xattr block 会全部漏掉 | 补 ibody 解析：entries 紧跟 4 字节 magic，`e_value_offs` 相对 entries 起点（对齐 e2fsprogs `read_xattrs_from_buffer`） |
| 解包出的大 APK 打不开、`zip` 报 `not a zip` / entry `read error` | `read_data` 按数组顺序拼接 extent，忽略了 `ee_block` 的逻辑空洞（extent 不连续时每个洞之后的数据整体前移 4096B，且文件尾部变短） | 按 logical block 推进：洞填 0，`ee_len > 32768` 按 unwritten extent 处理；解包后用 zip CRC 做验证 |
| 多块组 ext4（>128 MiB @4K）`e2fsck` 报 `bad block for block bitmap` / `Group N's block bitmap conflicts` | `g>=1` 的 block/inode bitmap 被放在 backup superblock + backup GDT 的块上，随后 backup 写入又把位图覆盖 | 布局改为 `SB + gdt_blocks` 之后；备份 SB 在 1 KiB 块下位于组首块、4 KiB 下位于块内偏移 1024 |
| `--block-size 1024` 组 0 报 `block bitmap at 2 conflicts` | `first_data_block=1` 没有参与 `g0_bb` 计算，位图压在主 GDT 上 | `g0_bb = first_data_block + 1 + gdt_blocks` |
| `super unpack` 双槽互相覆盖 | `_a`/`_b` 剥后缀成同名，后写的覆盖先写的 | 仅当基名唯一时剥后缀；输出路径重复直接 `bail` |
| vendor_boot v4 改分片后不生效 | 整段 ramdisk 仍取旧文件，表项偏移按新长度算 | 由分片重新拼接整段，表项偏移按同一份数据重算并校验长度 |
| `payload --compress` 不压缩、哈希错位 | `xz_compress` 的 Err 被 `.ok()` 吞掉；`data_sha256` 算的是压缩前数据 | 换 `xz2`（liblzma）并在失败时报错；哈希/长度按存储 blob 计算 |
| 读 ext2/ext3 文件尾部全 0 | 只遍历 12 个直接块，间接块未实现 | 实现单/双/三级间接块，按逻辑块号推进 |
| 差分 `.transfer.list` 解析失败 | 未支持 `bsdiff <off> <len>`、`move`、`stash/free` | 按 blockimg 语法解析，patch 用 `<off>/<len>` 定位 |
| boot v2/v4 `header_size` 偏小 | 写成 1648/1580 | v2=1660、v4=1584；v0–v2 补 `id[8]` SHA1，空段地址写 0 |
| meta 缺中间目录被拍平 | 补建父目录时父未创建就挂到根 | 递归 `ensure_dir` 自顶向下建 |
| 组边界尺寸构建失败 | 最后一组按满 inode table 占位 | 像 mke2fs 一样下调全局 `ipg`（`s_inodes_count == groups*ipg`） |
| depth-1 extent 叶子 `eh_max=4` | 复用了 inode 内根节点的写入函数 | 叶子/索引节点按 `(bs-12)/12` 写 `eh_max`，并支持 depth 2 |
| 稀疏镜像 `total_sz=0` 死循环 | `to_raw` 不校验 chunk 尺寸 | 强制 `total_sz >= 12` 并限制单 chunk 块数 |
| 截断 boot/vendor_boot 头 panic | 先切片后检查长度 | 按版本要求长度前置校验，返回错误 |
| bsdiff/payload 分配无上限 | 文件内 u64 直接 `vec!` | 用文件实际长度与合理上限约束 |
| 1 KiB 块构建 ≥8 MiB 文件失败 | 固定 8192 块连续分配，1K 组内放不下 | `alloc_run` 折半重试，按 run 生成多个 extent |
| `super --alignment 4096` 数据压进 backup metadata | `meta_end` 少算 4096 字节保留区 | `LP_PARTITION_RESERVED_BYTES + 2*GEOMETRY_SIZE + …` |
| 双槽 super 的 `_a` 永不剥后缀 | 冲突检测把被 `--slot` 过滤掉的 `_b` 也算进去 | 只统计实际导出的分区 |
| sparse 分区镜像被原样塞进 super | 未探测 sparse magic | 构建前 `file_is_sparse` 直接报错并提示 `sparse to-raw` |
| payload/镜像构建全量进内存 | `fs::read` + `extra_blocks` 持有整镜像 | 文件数据/压缩块流式写盘，160 MiB 文件峰值 RSS 5.8 MiB（原 334 MiB） |
| `ext4 extract` 大文件全量进内存 | `read_data` 返回整文件 `Vec` | 新增 `write_file_data` 按 extent 流式落盘（160 MiB 文件峰值 RSS 4.6 MiB） |
| payload 缺 `payload_properties.txt`、末块未补齐 | 只写 payload | 生成 `.properties`（FILE_HASH/FILE_SIZE/METADATA_HASH/METADATA_SIZE），op 按块补齐 |
| `--verify` 必须带值 | clap `Set` 要求 `--verify true` | 改为 `num_args=0..=1`，裸 `--verify` 即可 |
| 截断的 v3/v4 boot panic | 头部字段按 v0 布局无条件切片 | v≥3 用 44..1580，`extra_cmdline` 置空 |
| protobuf 长度 varint 溢出 panic | `self.p + n` 直接相加 | `checked_add` + 边界检查 |
| `s_blocks_per_group=0` / `s_inodes_per_group=0` 除零 panic | 未校验超级块 | parse 时拒绝 0 |
| `--size 300000000000G` 乘法溢出 panic | `v * mult` | `checked_mul` |
| 每文件 `sync_all` 导致解包 50 ms/文件 | 逐文件同步 | 移除；100 文件解包 6 ms |
| `extract` 条目插入 O(n²) | `MetaDb::insert` 线性查重 | 直接 push，末尾 sort + dedup |
| 每 inode 的 xattr/目录块全量驻留内存 | `extra_blocks` 缓存 | 生成后立即写盘 |
| 自动尺寸不看 inode 数 | 只按数据量估算 | 按 `ceil(nodes/ipg)` 组数兜底 |
| LP tables 4 GiB 分配 / entry_size=0 死循环 | 无上限校验 | tables ≤1 MiB，四类表 entry_size 下限校验 |
| payload 对 minor_version≠0 一律要求 `--old` | 看版本而非 op | 仅存在 SOURCE_COPY/SOURCE_BSDIFF 时要求 |
| properties 文件名 `payload.properties` | `with_extension` 截断 | 追加 `.properties`（`payload.bin.properties`） |
| `extract → build` 把 `meta.txt` 打进镜像 | `from_dir` 收录 | 跳过根目录 `meta.txt` |
| ext4 不识别 sparse 输入 | 直接按 raw 打开 | `info/ls/extract` 前自动展开 |
| 64 KiB 块组字节数溢出 | `bpg=8*bs=524288`（32 GiB） | 上限 4 GiB 内 8 对齐（65528） |

---

## 9. Python API（`python/pyaimg`）

Rust 负责字节级操作，Python 侧是带类型和 `.pyi` 存根的封装；权限模型（`FsConfig` / `FileContexts` / `MetaDb`）
在 Python 里也有一份纯 Python 实现，便于在不调用二进制时做校验和生成。

```bash
pip install -e python/
export AIMG_BIN=/path/to/aimg      # 或放到 PATH；Windows 为 aimg.exe
```

```python
import pyaimg as ai

# 文本权限（不看 NTFS）
fsc = ai.FsConfig.load("configs/fs_config.txt")
fc  = ai.FileContexts.load("configs/file_contexts")
assert fsc.lookup("/system/bin/sh") == (0, 2000, 0o755, 0)
assert fc.lookup("/system/bin/sh") == "u:object_r:shell_exec:s0"

# 目录树 -> ext4
ai.ext4.build("system_root/", "system.img",
              fs_config=fsc, file_contexts=fc, size="2G", label="system")
meta = ai.ext4.extract("system.img", "out/")
e = meta.get("/system/bin/sh")
print(e.uid, e.gid, e.mode_octal, e.context)

# super / payload / dat / boot
sup = ai.superimg.make("super.img",
        [ai.superimg.PartitionImage("system_a", "system.img", "main")], size="8G")
ai.superimg.unpack("super.img", "parts/", slot=0)

ai.payload.make("payload.bin", [("system", "system.img")])
print(ai.payload.info("payload.bin").partitions[0].op_types)
ai.payload.extract("payload.bin", "parts/")

ai.dat.split("system.img", "dat/", prefix="system", version=4)
ai.dat.merge("dat/system.transfer.list", "dat/system.new.dat", "system.raw")

b = ai.boot.info("boot.img")
ai.boot.unpack("boot.img", "boot_dir/")
ai.boot.pack("boot_dir/", "new-boot.img")
```

结构：

```
python/
├── pyproject.toml
├── README.md
├── pyaimg/
│   ├── __init__.py / __init__.pyi     # 导出与存根
│   ├── _proc.py  .pyi                 # 二进制定位 + AimgError + run/run_json
│   ├── types.py  .pyi                 # 全部 frozen dataclass（SuperInfo/PayloadInfo/...）
│   ├── perm.py   .pyi                 # FsConfig / FileContexts / MetaDb
│   ├── sparse.py superimg.py boot.py payload.py dat.py ext4.py pipeline.py (+ .pyi)
│   └── py.typed
└── tests/test_api.py                  # pytest 用例，打真二进制
```

设计约定：

- `super dump` / `payload info` / `boot info` / `ext4 info` / `dat info` 都支持 `--json`，Python 侧直接消费结构化输出；
- 所有路径参数接受 `str | os.PathLike`，返回 `pathlib.Path`；
- 失败统一抛 `pyaimg.AimgError`（带 `cmd/returncode/stdout/stderr`）；
- 结构化结果字段与 Rust `--json` 输出一一对应，`SuperInfo.size_of()` / `PayloadInfo.partition()` 等提供便捷访问；
- `super make` 的 `name=image[:group]` 在 Windows 上做了盘符保护（`F:\...` 不会被当成 group 分隔）。

## 7. Skill：给 AI 用的镜像操作手册

`skills/aimg-image-ops/` 是一个可加载的 skill（`SKILL.md` + `references/`），内容覆盖
unpack / 修改 / 重打包 / 校验的完整流程与踩坑表，适合直接交给 AI 代理执行镜像操作。

- 入口：`skills/aimg-image-ops/SKILL.md`
- 命令参考：`skills/aimg-image-ops/references/cli.md`
- 格式细节：`skills/aimg-image-ops/references/formats.md`
- 故障排查：`skills/aimg-image-ops/references/troubleshooting.md`

安装到用户级 skill 目录（二选一）：

```bash
# 复制（Windows: %USERPROFILE%\.agents\skills）
cp -r skills/aimg-image-ops ~/.agents/skills/
# 或让代理直接从仓库加载该目录
```

加载后代理会按 SKILL.md 的规则工作：先备份/哈希、按分区尺寸构建、用 `e2fsck` 和回读校验、
权限与 SELinux 只从文本配置来。

## 8. CI：私有仓库发版 → GitHub 构建发布

- 私有侧（Gitea Actions）：Release 发布后，把该 Release 的源码 tarball 作为**一个 commit**
  同步到 GitHub；同步时删除 `.gitea/`，私有 Runner 配置不会出现在 GitHub。
  工作流：`.gitea/workflows/mirror-release-to-github.yml`，逻辑：`scripts/ci/mirror_release_to_github.sh`。
- GitHub 侧：push 后自动构建 **20 个 target**（Linux/Windows/macOS/FreeBSD/Android，x86_64/i686/aarch64/armv7/arm/riscv64/powerpc64le），并创建 Release。
  工作流：`.github/workflows/build-release.yml`。
- 需要的 Secret/Variable、触发方式与本地演练：见 `docs/ci.md`。

```text
Release(tag) --Gitea Actions--> tarball --> 单次 commit --> GitHub push --> test/build --> Release(两个包)
```
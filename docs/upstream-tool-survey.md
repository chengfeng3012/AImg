# Android 镜像工具链选型（源码可信度核查版）

核查时间：2026-11-09。所有仓库均实际抓取过（GitHub API / raw / googlesource `?format=TEXT` 目录列表），
标注的 star 数、最后 push 时间来自本次核查。

---

## 0. 结论速览

| 需求 | 首选（权威/可靠） | 备选 | 语言 / 平台 |
|---|---|---|---|
| super.img 解包+回包 | AOSP `system/extras/partition_tools`（lpunpack/lpmake/lpdump/lpadd/lpflash） | `Rprop/aosp15_partition_tools`（预编译 Win/Linux）、`unix3dgforce/lpunpack`（纯 Py，仅解包）、`LonelyFool/lpunpack_and_lpmake` | C++ / Linux、Win 预编译 |
| payload.bin 解包 | `ssut/payload-dumper-go`（3.6k★，2026-09，Apache-2.0） | `vm03/payload_dumper`（1.8k★）、`5ec1cff/payload-dumper`（852★）、`EmilyShepherd/ota-tool`（C++）、`libxzr/FastbootEnhance`（Win GUI） | Go / Py / C++ / C# |
| payload.bin 回包 | AOSP `system/update_engine` 的 `delta_generator`（`scripts/brillo_update_payload` 驱动） | `snowwolf725/Payload_Repack_Tool`（封装+签名，68★）、`rhythmcache/payload_packer`（Rust 独立实现，18★，实验性） | C++ / Py / Rust |
| .dat 差分包 | `xpirt/sdat2img`（930★）+ `erfanoabdi/imgpatchtools`（201★） | 反向打包 `xpirt/img2sdat`（168★） | Python / C |
| boot/recovery.img | `cfig/Android_boot_image_editor`（Java，AVB 重签）或 AOSP `system/tools/mkbootimg` | `magiskboot`（Magisk native）、`osm0sis/Android-Image-Kitchen`（Win 一键） | Java / Py / C++ |
| 稀疏镜像互转 | AOSP `system/core/libsparse`（simg2img / img2simg / append2simg） | `anestisb/android-simg2img`（独立 Makefile）、`dlenski/PySIMG`（纯 Py，支持 split） | C / Python |
| 纯用户态 ext 解包+回包 | `e2fsdroid`（AOSP `external/e2fsprogs/contrib/android`）+ `debugfs` | `make_ext4fs`（已弃用但可用）、`fuse2fs`、`e2tools` | C |

**平台建议**：本机是 Windows，AOSP 那批工具（lpmake、delta_generator、e2fsdroid、libsparse）在 WSL2 Ubuntu 22.04/24.04
里直接 `apt` + `mm` 最省事；Windows 原生只用来跑预编译件和 Java 版 boot 编辑器。

---

## 1. super.img（动态分区，LP metadata）

### 1.1 权威源码（唯一正解）

AOSP `system/extras/partition_tools`，本次已拉目录确认存在：
`lpmake.cc` / `lpunpack.cc` / `lpdump.cc` / `lpadd.cc` / `lpflash.cc` / `lpdumpd.cc` + `aidl/`。

- Web：https://android.googlesource.com/platform/system/extras/+/refs/heads/main/partition_tools/
- 克隆（只取该目录，几百 MB 仓库不要全拉）：

```bash
git clone --depth 1 --filter=blob:none --sparse \
  https://android.googlesource.com/platform/system/extras
cd system/extras && git sparse-checkout set partition_tools
# 或者直接 mm 编译（需要 AOSP 树）：
m lpunpack lpmake lpdump lpadd
```

没有 AOSP 树时的两条实用路径：

- `Rprop/aosp15_partition_tools`（12★，2025-04）：从 android-15.0.0_r25 静态编译好的
  `lpmake/lpunpack/lpdump/ext2simg/simg2img`，Linux + Windows 都有，省掉整棵 AOSP。
  https://github.com/Rprop/aosp15_partition_tools
- `LonelyFool/lpunpack_and_lpmake`（160★，2023-07）：AOSP 源码 + `make.sh`，`apt install clang zlib1g-dev` 即可，
  比整树编译轻。
  https://github.com/LonelyFool/lpunpack_and_lpmake

纯解析备选：`unix3dgforce/lpunpack`（224★，2025-03，Python）——不依赖编译，能直接列出/抽取 super 内分区，
但**只解不打**，适合先看清 layout。
https://github.com/unix3dgforce/lpunpack

### 1.2 解包 / 回包命令

```bash
lpdump super.img                      # 先看 metadata：slot 数、group、每条分区的 extents
lpunpack --slot=0 super.img ./out     # 解出 system_a.img / vendor_a.img ...
lpunpack super.img ./out              # 不带 --slot 则按分区名原样输出（含 _a/_b）
```

回包必须显式重建 metadata，`--device` 的 size 要 ≥ group 总和：

```bash
lpmake --metadata-size 65536 \
       --super-name super \
       --metadata-slots 2 \
       --device super:9126805504 \
       --group main:9122611200 \
       --partition system:readonly:1073741824:main --image system=system_a.img \
       --partition vendor:readonly:536870912:main  --image vendor=vendor_a.img \
       --output super_new.img
```

要点/坑：

- `--metadata-slots` 必须与原 super 一致（`lpdump` 里 `Metadata slots`），否则设备侧 `fs_mgr` 读不到。
- `--device super:SIZE` 是**整个块设备**大小；`--group main:SIZE` 是可用数据区，两者差值就是 metadata 区。
- 分区顺序、`readonly`/`none` 属性、group 名（`main`、`qti_dynamic_partitions` 等）都要照抄，否则开机 dm-linear 映射失败。
- super 里是**逻辑分区原始数据**，不含 vbmeta；AVB 在分区尾部 footer 或独立 `vbmeta.img`，改完分区内容必须重签（见 §7）。
- 输出若要 sparse：`img2simg super_new.img super_new.sparse.img`。
- 常见翻车：回包后 `lpdump` 能读但设备起不来 —— 90% 是 `--metadata-slots` 或 group size 写错，或者漏了 `--partition x:none:0:main` 的空占位分区。

---

## 2. payload.bin（A/B OTA）

### 2.1 解包（含增量差分）

**首选 `ssut/payload-dumper-go`**（3.6k★，2026-09-23 仍在推，Apache-2.0）：

https://github.com/ssut/payload-dumper-go

它的可信点不在 star，在实现：Go 并行 xz/bz2/zstd 解压、**校验每个 operation 的 data/source sha256**、
校验失败非零退出、增量 OTA 用 `-old` 在 base 镜像上做 bit-exact 还原、并且**自己重建 dm-verity hash tree + FEC**
（不依赖 avbtool / fec 工具），还能直接读 zip 不落地临时文件。

```bash
payload-dumper-go -l payload.bin                     # 列分区
payload-dumper-go -o out -p boot,system,vendor ota.zip
payload-dumper-go -o base base_full.zip              # 先取 base
payload-dumper-go -old base -o out incremental_ota.zip   # 增量还原
```

备选：

- `vm03/payload_dumper`（1.8k★，2025-04，Py）：`python payload_dumper.py --diff payload.bin`，老牌、纯 Python、Docker 镜像现成。
  https://github.com/vm03/payload_dumper
- `EmilyShepherd/ota-tool`（32★，C++ 重写自 payload-dumper-go）：自带 `--input` 先生目录做 patch，适合"只改一个分区"的流程。
  https://github.com/EmilyShepherd/ota-tool
- `5ec1cff/payload-dumper`（852★，Py，2026-04）：解析更"裸"，适合研究 manifest 结构。
- `libxzr/FastbootEnhance`（1.3k★，C#）：Windows GUI，给不写命令行的人用。
  https://github.com/libxzr/FastbootEnhance

### 2.2 回包（生成 payload.bin）

官方路径 —— AOSP `system/update_engine`（本次确认：`payload_generator/` 下
`generate_delta_main.cc`、`delta_diff_generator.cc`、`full_update_generator.cc`，`scripts/brillo_update_payload`、
`scripts/payload_info.py` 都在 main 上）：

```bash
# 1) AOSP 树内编译
m delta_generator
# 2) 用官方脚本驱动（自动拼 --new_partitions / --old_partitions / minor_version / 压缩参数）
./scripts/brillo_update_payload generate \
  --payload=payload.bin \
  --target_image=target_images.zip \
  --source_image=source_images.zip \
  --signature_size=267
# 3) 校验
./scripts/payload_info.py --payload payload.bin --json
```

不想搭 AOSP 的两种可行做法：

1. **`snowwolf725/Payload_Repack_Tool`**（68★，2026-05，MIT，Py 封装 `delta_generator` + 预编译二进制 + 签名 key）：
   ```bash
   cp *.img IMAGES/ && ./setup.sh          # 自动生成 META/dynamic_partitions_info.txt、ab_partitions.txt
   ./repackPayload.sh                      # 出 payload.bin
   ./repackZip.sh                          # 出带签名的 OTA zip
   ```
   自带 `module-CustomOTA_CA.zip`，用于把设备上的 OTA 校验公钥换成自己的，否则重签的包会被拒。
   https://github.com/snowwolf725/Payload_Repack_Tool
2. **`rhythmcache/payload_packer`**（18★，Rust，Apache-2.0，2025-12）：`cargo install` 即用，不依赖 AOSP/Java。
   支持 full 与 incremental（ZERO / SOURCE_COPY / SOURCE_BSDIFF / REPLACE 四种 op 自动择优），
   已被 payload_dumper / payload-dumper-go 反向验证过逐 bit 一致；但作者自己标注实验性、**不签名**。
   https://github.com/rhythmcache/payload_packer

坑：

- `delta_generator` 的 `--minor_version`、`--max_timestamp`、`--block_size` 要与原包一致，否则设备端 `update_engine` 校验/应用失败。
- 签名是 payload 级 RSA（`payload_metadata.bin` 尾部 signature），换 key 就必须配 Magisk 模块或走 TWRP。
- 增量包必须提供与设备当前分区 **sha256 完全一致**的 old 镜像，否则 SOURCE_* op 直接失败。

---

## 3. .dat（块级 OTA 数据 / 差分包）

分两种东西，别搞混：

| 文件 | 含义 | 工具 |
|---|---|---|
| `system.new.dat` + `system.transfer.list` | 全量块映射数据流 | `sdat2img`（→ img） |
| `system.patch.dat` + 同目录 `.new.dat` | 增量补丁流（BlockImageUpdate） | `imgpatchtools` 的 `BlockImageUpdate` |
| `*.p` | boot/firmware 的 bsdiff 补丁 | `imgpatchtools` 的 `ApplyPatchfn` |
| `system.new.dat.br` | 上面 `.dat` 的 brotli 压缩 | 先 `brotli -d`，再走 sdat2img |

**`xpirt/sdat2img`**（930★，2025-12 仍在动，MIT，纯 Python 单文件，Win/Linux/macOS/ARM 全支持）：
https://github.com/xpirt/sdat2img

```bash
brotli -d system.new.dat.br -o system.new.dat     # 若为 .br
python sdat2img.py system.transfer.list system.new.dat system.img
```

**`erfanoabdi/imgpatchtools`**（201★，2023-10，C，需 zlib/libbz2/openssl）：把 AOSP recovery 里的
`blockimg.cpp` + `applypatch` 逻辑抽成独立 CLI，是 PC 上复现差分包合并最直接的实现。
https://github.com/erfanoabdi/imgpatchtools

```bash
# 在 base 镜像上原地打增量（base 必须是该 OTA 的 source 版本）
./BlockImageUpdate system.img system.transfer.list system.new.dat system.patch.dat
# bootstrap 类补丁
./ApplyPatchfn boot.img - <tgt_sha1> <size> <init_sha1> <patch.p>
```

反向（改完 img 想吐回 .dat，给老设备/老 recovery 用）：`xpirt/img2sdat`（168★，MIT）
`python img2sdat.py system.img -o out -v 4 -p system`。
https://github.com/xpirt/img2sdat

坑：

- transfer list 有 v1–v4，`img2sdat -v` 选错版本刷机直接报 `E1001`。
- `imgpatchtools` 的 `BlockImageUpdate` 是**原地写**，先 `cp` 一份 base。
- 这类 `.dat` 格式基本只存在于 Android 8 及更早（A-only 时代），新机全是 payload，别拿错工具。

---

## 4. boot.img / recovery.img（含 vendor_boot、dtbo）

### 4.1 首选：`cfig/Android_boot_image_editor`

https://github.com/cfig/Android_boot_image_editor

Apache-2.0，Gradle + Java（JDK17+），CI 在跑，README 明确覆盖：
`boot.img / init_boot.img / vendor_boot.img / vendor_kernel_boot.img / recovery.img / vbmeta*.img / dtbo.img / dtb / sparse 镜像 / payload.bin`。
价值在于它**按 AOSP verified boot 流程重建**：解包出 `boot.json`（原 header 全部参数）+ `boot.avb.json`，
改完 `pack` 会按原参数重打包并重算 AVB 签名（`boot.img.signed`）—— 这是自己手拼 `mkbootimg` 最容易漏的一环。

```bash
cp 原boot.img boot.img
./gradlew unpack         # → build/unzip_boot/{boot.json,kernel,dtb,root/...}
# 改文件
./gradlew pack           # → boot.img.signed
```

### 4.2 权威参数级：AOSP `system/tools/mkbootimg`

本次确认 main 分支存在 `mkbootimg.py`、`unpack_bootimg.py`、`repack_bootimg.py`。
https://android.googlesource.com/platform/system/tools/mkbootimg/

```bash
python unpack_bootimg.py --boot_img boot.img --out boot_out     # 打印 header 全字段 + 落盘 kernel/ramdisk
# 按打印出的 pagesize/os_version/header_version/cmdline 原样回灌：
python mkbootimg.py --kernel boot_out/kernel --ramdisk boot_out/ramdisk \
  --cmdline "..." --pagesize 4096 --header_version 4 \
  --os_version 14.0.0 --os_patch_level 2024-05 --output new-boot.img
# 或者直接 repack：
python repack_bootimg.py --boot_img boot.img --out new-boot.img
```

这是 header v0–v4 的**规范实现**，其它工具出问题时的对照基准。

### 4.3 最快：`magiskboot`（Magisk native）

仓库：https://github.com/topjohnwu/Magisk（63.9k★，2026-10 活跃）。native 部分已 Rust 化，
`native/src/boot/` 下可见 `bootimg.cpp / cpio.rs / dtb.rs / zimage.rs / compress.rs / sign.rs / patch.rs`。

```bash
magiskboot unpack boot.img
magiskboot cpio ramdisk.cpio "extract"      # 看/改 cpio
magiskboot dtb dtb.img print
magiskboot repack boot.img new-boot.img
```

优点：单文件静态二进制、处理各种 vendor 魔改 header 和压缩格式（lz4/gzip/zstd/lzma）最鲁棒。
注意：它是为"改 ramdisk 打 Magisk"设计的，`payload.rs` 只用于从 OTA 里抠 boot，**不要拿它当通用 payload 工具**。

### 4.4 Windows 一键：`osm0sis/Android-Image-Kitchen`

https://github.com/osm0sis/Android-Image-Kitchen （1.1k★，2023-04）
根目录就是 `unpackimg.bat` / `repackimg.bat` / `cleanup.bat` + `android_win_tools/`，
双击即可，适合快速看 ramdisk 内容；不适合精细控制 header 字段。

坑：

- `recovery.img` 的 `--recovery_dtbo`（v1/v2 header）与 `vendor_boot` 的 ramdisk table（v4）必须保留原值，
  只换 kernel 时尤其容易把 `dtb` 段丢掉 → 开机黑屏/无限重启。
- 改完 boot 必须处理 AVB：`avbtool add_hash_footer`（或编辑器自动做），否则 dm-verity 报 `verification failed`。
- `boot.img` 大小不能超过分区；`mkbootimg` 不打 padding 时 fastboot 会拒绝。

---

## 5. 稀疏镜像（Android sparse image）互转

### 5.1 规范实现：AOSP `system/core/libsparse`

本次确认 main 分支含 `simg2img.cpp`、`img2simg.cpp`、`append2simg.cpp`、`simg_dump.py`、`sparse_format.h`。
https://android.googlesource.com/platform/system/core/+/refs/heads/main/libsparse/

```bash
simg2img system.img system.raw.img          # sparse → raw
img2simg system.raw.img system.simg 4096    # raw → sparse（可指定块大小）
simg_dump.py system.img                     # 看 chunk 结构
```

AOSP 树内：`m simg2img img2simg append2simg`；`system/extras/ext4_utils` 另提供 `ext2simg`。

### 5.2 独立可编译：`anestisb/android-simg2img`

AOSP libsparse 的独立 port，`make` 即出 `simg2img` / `img2simg` / `simg2simg`，README 给了 MinGW 编 Windows 的方法。
https://github.com/anestisb/android-simg2img

```bash
make
CFLAGS=-DUSE_MINGW LDFLAGS=-lasprintf mingw32-make    # Windows
```

### 5.3 纯 Python：`dlenski/PySIMG`

https://github.com/dlenski/PySIMG —— 支持流式转换、`--split MiB` 切多卷 sparse（老设备 `system.img_sparsechunk.*` 场景），
`-D 00000000` 把全零块标 DONT_CARE。作者声明默认模式与官方 `img2simg` 输出逐 bit 一致。

坑：

- 多卷 sparse（`system.img_sparsechunk.0..N`）合并：`simg2img system.img_sparsechunk.* super.img`（simg2img 支持多输入）。
- sparse 头里的 `blk_sz` 必须是 4096（少数设备 8192），转 raw 后再 `img2simg 4096` 回写最稳。
- `lpmake` 产出的是 raw，要 sparse 必须再过一次 `img2simg`。

---

## 6. 纯用户态 ext2/3/4 解包 + 回包（不 mount、不 root）

### 6.1 回包正解：`e2fsdroid`

AOSP `external/e2fsprogs/contrib/android/e2fsdroid.c`（本次确认存在）。
它不是"打包工具"，而是**把目录树写进 ext4 镜像**，并按 Android 规则落 SELinux context / uid-gid / mode：

```bash
e2fsdroid -e -a /system \
  -S file_contexts \      # 来自 ROM 的 system/etc/selinux 或 AOSP out
  -C fs_config.txt \      # 来自 ROM 的 fs_config（bin/、etc/ 等权限表）
  -f ./system_root \
  system.img
```

AOSP 里对应的是 `system/extras/ext4_utils/mkuserimg_mke2fs.py`（本次确认 main 上仍在），
它负责算大小、建 fs、调 e2fsdroid：

```bash
mkuserimg_mke2fs.py system_root system.img ext4 system 3221225472 \
  -j 0 -T 1230768000 -C fs_config.txt -B 4096 -L system \
  --fs_config_system ... --selinux_file_contexts ...
```

### 6.2 解包：`debugfs`（e2fsprogs 官方，`external/e2fsprogs/debugfs/` 已确认）

```bash
e2fsck -fn system.img                       # 先自检
debugfs -R "rdump / ./system_root" system.img
debugfs -R "ls -l /system/priv-app" system.img
```

回写单个文件：`debugfs -w -R "write ./foo /system/foo" system.img`（注意：**不自动改权限/xattr**，
需要 `sif` / `set_inode_field` 手工补，或者干脆走 6.1）。

### 6.3 其它

- `make_ext4fs`（AOSP `system/extras/ext4_utils`，已弃用）：`-S file_contexts -C fs_config -a system -l 2G`，
  老 ROM 仍在用；预编译版 `rendiix/make_ext4fs`（含 `img2simg`/`simg2img`，Apache-2.0）。
  https://github.com/rendiix/make_ext4fs
- `fuse2fs`（e2fsprogs `misc/fuse2fs.c`，已确认）：用户态 FUSE 挂载，**xattr 原生保留**，
  改完 `umount` + `e2fsck -f`。需要 `/dev/fuse` 可用（WSL2 默认没有，Linux 桌面可用）。
  `fuse2fs -o fakeroot system.img /mnt/img`
- `e2tools`（40★，C，GPL）：`e2cp/e2ls/e2mkdir` 在镜像内直接读写，无需 root，适合脚本化小改动。
  https://github.com/e2tools/e2tools

### 6.4 关键坑（这是"纯用户态"最容易翻车的地方）

1. **SELinux context 丢失**：`debugfs rdump` 不导出 `security.selinux` xattr，回包后所有文件变成
   `u:object_r:unlabeled:s0` → 开机 `init` 拒绝。所以要 `e2fsdroid -S file_contexts -C fs_config`，
   而不是 `debugfs` 来回倒。
2. **大小/预留**：改前先 `resize2fs system.img 3G`，留 5%–10% 余量；回包后 `resize2fs -M` 缩到最小再按分区表对齐。
3. **symlink / 硬链 / capability**：用 `tar --xattrs --numeric-owner` 在宿主和镜像根目录之间中转，
   否则 ext4 里的 symlink 会在 Windows 文件系统上被展开/丢失。
4. **journal**：`e2fsdroid` 默认 `-j 0`（无日志），与原厂一致；手工 `mke2fs` 记得 `-O ^has_journal` 或 `-j` 对齐原值。

---

## 7. 收尾必做：AVB / verity / 校验

```bash
# 重算分区 hash footer（改过 system/vendor/boot 之后）
avbtool add_hash_footer --image system.img --partition_name system \
  --partition_size 3221225472 --key avb_key.pem --algorithm SHA256_RSA2048
# 重做 vbmeta（链式描述符，含 chain partition）
avbtool make_vbmeta_image --output vbmeta.img --key avb_key.pem \
  --algorithm SHA256_RSA2048 --chain_partition system:1:avb_system.pem ...
```

`avbtool.py` 在 AOSP `external/avb/avbtool.py`（本次确认存在）。

校验顺序建议：

1. `lpdump super_new.img` / `lpunpack` 回读，确认分区表与 extents 一致；
2. `e2fsck -fn` 每个 ext4 分区；
3. `simg2img` → `file` 确认 `Linux rev 1.0 ext4 filesystem data`；
4. payload 用 `payload_info.py` 或 `payload-dumper-go -l` 复核 manifest；
5. 设备侧先 `fastboot boot`/临时 recovery 验证，再落盘。

签名现实：任何改过的镜像都过不了原厂 AVB；要么自签 + 关 verity（`fastboot --disable-verity --disable-verification flash vbmeta`），
要么走 TWRP/Magisk 路线。OTA zip 还需要原厂私钥，否则只能用 `Payload_Repack_Tool` 那套换 key 方案。

---

## 8. 一次性拉取脚本（WSL2 / Linux）

```bash
mkdir -p ~/android-img && cd ~/android-img

# 1) AOSP 关键仓（稀疏拉取，只取需要的目录）
for r in system/extras system/tools/mkbootimg system/core system/update_engine external/e2fsprogs external/avb; do
  n=$(echo $r | tr '/' '_')
  git clone --depth 1 --filter=blob:none --sparse https://android.googlesource.com/platform/$r $n
done
cd system_extras && git sparse-checkout set partition_tools ext4_utils && cd ..
cd system_core   && git sparse-checkout set libsparse && cd ..
cd system_update_engine && git sparse-checkout set payload_generator scripts && cd ..
cd external_e2fsprogs && git sparse-checkout set contrib/android debugfs misc && cd ..

# 2) GitHub 工具
git clone --depth 1 https://github.com/ssut/payload-dumper-go
git clone --depth 1 https://github.com/cfig/Android_boot_image_editor
git clone --depth 1 https://github.com/xpirt/sdat2img
git clone --depth 1 https://github.com/xpirt/img2sdat
git clone --depth 1 https://github.com/erfanoabdi/imgpatchtools
git clone --depth 1 https://github.com/anestisb/android-simg2img
git clone --depth 1 https://github.com/snowwolf725/Payload_Repack_Tool
```

编译要点：`delta_generator` 需要 `protobuf` + `openssl` + `brotli`/`bz2`/`lzma` 开发包；
`e2fsdroid` 直接 `./configure --enable-android && make` 即可（`misc/` 里会一起出 `fuse2fs`）；
`lpunpack` 用 `LonelyFool/lpunpack_and_lpmake` 的 `make.sh` 最快（clang + zlib）。

---
name: aimg-image-ops
description: Unpack, inspect, modify and rebuild Android partition images with the pure-Rust `aimg` toolchain (Windows/Linux, no root, no mount, no loop device, no NTFS ACLs). Covers sparse images, super.img / dynamic partitions (LP metadata), boot/recovery/vendor_boot, payload.bin (A/B OTA), legacy .dat OTA packages, and ext4 read/build with uid/gid/mode/SELinux taken from text configs (fs_config, file_contexts, meta.txt). Use when a task involves Android images, OTA packages, super/dynamic partitions, ext4 trees, or preserving SELinux contexts and file capabilities while repacking.
license: GPL-3.0-only
---

# aimg image operations

`aimg` is a self-contained Rust binary. Every operation is plain file I/O: it never mounts
anything, never needs root, and never stores permissions in NTFS ACLs — ownership, mode,
SELinux context and capabilities live in `meta.txt` (a TSV) and are baked into the ext4 image
as real `security.selinux` / `security.capability` xattrs.

## 0. Find the tool

Prefer, in order:

1. `aimg` on `PATH`;
2. `<repo>/target/release/aimg.exe` (Windows) or `<repo>/target/release/aimg` (Linux);
3. the snapshot binary under `dist/<date>/aimg.exe`.

Run `aimg --help` before scripting; the CLI is stable but flags are explicit.

## 1. Golden rules

- **Work on copies.** Treat every source image and OTA as read-only input; write to a new
  directory. Keep the original hashes (`sha256sum`) for comparison.
- **Sizes are part of the format.** An ext4 image must be built with the exact partition size
  (`stat -c %s original.img`) or it will not fit the super metadata / device partition.
- **Verify with an independent reader.** `e2fsck -fn image.img` on Linux (container is fine) plus
  a read-back (`ext4 extract` + `cmp`) is the minimum bar. `aimg` itself has no fsck.
- **Never trust names from inside an image.** `aimg` rejects `..`, separators, `:`, control
  characters and Windows device names in paths taken from metadata; do not "fix" that by
  bypassing it.
- **Permissions come from text.** If uid/gid/mode/SELinux matter (Android system images), use
  `meta.txt` / `fs_config` / `file_contexts`, not the host filesystem.
- Prefer JSON output (`--json`) when a script needs to parse `super dump`, `payload info`,
  `boot info`, `ext4 info`, `dat info`.

## 2. Pick the workflow

| Input / goal | Start with |
|---|---|
| `*.simg` / sparse image | `aimg sparse info`, `aimg sparse to-raw` |
| `super.img`, dynamic partitions | `aimg super dump`, `aimg super unpack` |
| `boot.img` / `recovery.img` / `vendor_boot.img` | `aimg boot info`, `aimg boot unpack` |
| `payload.bin` (A/B OTA) | `aimg payload info`, `aimg payload extract --verify` |
| legacy OTA `*.new.dat.br` + `*.transfer.list` | `aimg dat br` → `aimg dat merge` |
| raw ext4 image | `aimg ext4 info`, `aimg ext4 extract` |
| a directory tree → ext4 image | `aimg perm export` (optional) → `aimg ext4 build` |
| modified partitions → super | `aimg ext4 build` ×N → `aimg super make` |

Full flag reference: `references/cli.md`.
Format details: `references/formats.md`.
Known pitfalls and fixes: `references/troubleshooting.md`.

## 3. Standard workflows

### 3.1 Full OTA (`.dat.br` + `transfer.list`) → trees

```bash
unzip -o ota.zip 'system.new.dat.br' 'system.transfer.list' -d ota/
aimg dat br ota/system.new.dat.br --out ota/system.new.dat
aimg dat merge ota/system.transfer.list ota/system.new.dat --out out/system.img
aimg ext4 extract out/system.img --out tree/system
```

Repeat per partition. `dat merge` honours `transfer.list`'s `total_blocks`, so trailing
zero blocks are preserved.

### 3.2 `super.img` → partition images → trees

```bash
aimg super dump super.img            # layout, groups, extents, sizes
aimg super unpack super.img --out parts/ --slot 0
aimg ext4 extract parts/system.img --out tree/system
```

`--slot 0` exports unsuffixed and `*_a` partitions; `--slot 1` exports unsuffixed and `*_b`.
Suffixes are only stripped when the base name is unique among exported partitions.
`--keep-suffix` keeps the original names.

### 3.3 Tree → ext4 → super (repack)

```bash
# size must equal the original partition image size
aimg ext4 build --dir tree/system --meta tree/system/meta.txt \
    --out out/system.img --size 1700110336 --label system --block-size 4096

aimg super make --out out/super.img --size 8589934592 \
    --metadata-max-size 65536 --metadata-slots 2 --alignment 1048576 \
    --partition system_a=out/system.img \
    --partition vendor_a=out/vendor.img \
    --partition product_a=out/product.img
```

Keep the original `--metadata-max-size`, `--metadata-slots`, `--alignment`, partition order and
sizes from `super dump`; changing them changes the on-device layout.

### 3.4 boot / vendor_boot

```bash
aimg boot info boot.img --json
aimg boot unpack boot.img --out boot/
# edit kernel/ramdisk/dtb/bootconfig or boot.json
aimg boot pack boot/ --out boot-new.img
```

`vendor_boot` v4 fragments (`vendor_ramdisk.<name>`) are concatenated again on pack, and the
ramdisk table entries are recomputed from the actual fragment sizes.

### 3.5 payload.bin

```bash
aimg payload info payload.bin --json
aimg payload extract payload.bin --out out/ --verify
aimg payload make --out payload-new.bin --partition boot=boot.img --compress
```

`extract --verify` checks each stored blob hash and the whole-partition hash. Only
`REPLACE` / `REPLACE_BZ` / `REPLACE_XZ` / `ZERO` / `DISCARD` / `SOURCE_COPY` /
`SOURCE_BSDIFF` are supported; PUFFDIFF / ZUCCHINI / LZ4DIFF fail loudly.

### 3.6 Edit permissions without a Linux host

```bash
aimg perm export --dir tree/system --out tree/system/meta.txt \
    --fs-config configs/fs_config.txt --file-contexts configs/file_contexts
aimg perm show --meta tree/system/meta.txt /system/bin/sh
# edit meta.txt by hand if needed, then
aimg ext4 build --dir tree/system --meta tree/system/meta.txt --out system.img --size ...
```

`meta.txt` is tab-separated and 11 columns:
`kind path uid gid mode context caps size mtime link_target ino`.
`ino` is optional and only used to preserve hard links across extract/build.

## 4. Verification checklist

1. `sha256sum` of inputs before work; compare rebuilt partition images against `super unpack`
   output when checking a super build.
2. `e2fsck -fn <img>` (Linux) for every built ext4 image — exit 0 required.
3. `aimg ext4 extract` one image back and `cmp` a few representative files (largest APK, an ELF
   in `bin/`, `build.prop`).
4. `aimg super dump` the rebuilt super: partition sizes/order/extents must match the original.
5. `aimg payload extract --verify` for payloads; `unzip -t` plus a sample file hash for zips.

## 5. Limits

- No AVB/vbmeta signing or verification; modifying a partition invalidates dm-verity/AVB.
- No PUFFDIFF / ZUCCHINI / LZ4DIFF delta support.
- ext4 writer supports a practical subset (extents, inline/external xattr, hard links, dirs,
  symlinks); it does not create inline-data files, htree indexes, metadata_csum or 64bit
  features.
- BSDF2 (`BSDF2` magic) patches are rejected; classic `BSDIFF40` is supported.
- Windows cannot create real symlinks without developer mode; `ext4 extract` falls back to a
  regular file holding the target, which `ext4 build` understands via `meta.txt`.

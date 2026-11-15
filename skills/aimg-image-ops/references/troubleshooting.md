# Symptom → cause → fix

All of these were real bugs found by adversarial review; current binaries already contain the
fixes. If a symptom reappears, first check that you are running the current binary.

| Symptom | Cause | Fix |
|---|---|---|
| Extracted large APK/JAR fails `zip` (`not a zip`, entry read error) | ext4 reader ignored logical holes between extents | current `ext4 extract` materialises holes; re-extract, compare `sha256` with the in-image copy |
| `e2fsck`: `bad block for block bitmap` / bitmap conflicts at group ≥1 | group bitmaps placed on backup SB/GDT blocks | current builder reserves `SB+GDT` first; rebuild |
| `--block-size 1024` cannot build an ≥8 MiB file | fixed 8192-block contiguous allocation vs 8 MiB groups | current builder halves the request and emits one extent per run |
| `e2fsck`: superblock corrupt on 64 KiB blocks | `blocks_per_group = 8*bs` overflowed 4 GiB groups | current builder caps bpg (65528 at 64 KiB) |
| `ext4 build` fails `no space left` although tree looks smaller | sparse files in the source were expanded; or xattr block per inode | current builder preserves holes and writes xattrs inline; also check tree vs partition size |
| `inode table exhausted` without `--size` | auto-size ignored inode count | current builder sizes by `ceil(nodes/ipg)`; still pass `--size` equal to the real partition |
| `e2fsck`: unattached inode / directory is a file | meta.txt maps a path with children as a file | fix `meta.txt`; builder now refuses that combination |
| Rebuilt image has `unlabeled` SELinux | reader only looked at external xattr blocks | current reader parses inode-body xattrs; use `ext4 extract` from the current binary |
| `payload --verify` passes but output is wrong | `data_sha256` used to cover pre-compression data | current `make` hashes the stored blob; rebuild payload |
| payload-dumper reports `operation data hash mismatch` | payload built by an old binary | rebuild with current `payload make` |
| `unsupported type 9/11/12/13` on payload extract | PUFFDIFF / ZUCCHINI / LZ4DIFF not implemented | use a full payload, or the vendor's delta tooling |
| `BSDF2 patches are not supported` | Android 10+ compressed bsdiff | extract a full payload instead |
| vendor_boot v4 fragment unpacks empty after editing | ramdisk table not rebuilt from fragments | current `boot pack` rebuilds the whole section and offsets |
| `super unpack` produced one file for A and B | suffix stripping collided | current code strips only unique base names; use `--keep-suffix` to keep names |
| `super make` baked a sparse image as data | sparse partition passed directly | current `super make` refuses; run `sparse to-raw` first |
| boot images repacked with wrong `header_size` / missing `id` | v2/v4 constants and v0–v2 hash | current builder writes 1660/1584 and SHA1 |
| `dat info` fails on a real differential OTA | old parser expected two rangesets and a u32 length prefix | current parser implements blockimg syntax; use `dat patch` for diffs |
| `dat merge` output shorter than the partition | size taken from last command only | current merge uses `max(total_blocks, last block)` |
| `ext4 build` panics / divide by zero on a weird image | malformed superblock | current parser rejects `blocks_per_group=0` / `inodes_per_group=0` |
| Windows extract creates regular files instead of symlinks | no developer mode / privileges | expected; target is preserved in `meta.txt` and restored on build |
| `meta.txt` inside the tree gets baked | building from a tree without `--meta` | current `perm export`/build skip root `meta.txt`; prefer explicit `--meta` |
| Repack of a dual-slot super yields `system.img` only | `--slot 0` filters `_b` | intended; use `--slot 1` or `--keep-suffix` for the other slot |

## Debugging checklist

1. `aimg <cmd> --help` — flags change between releases.
2. `aimg ... --json` — machine-readable layout/manifest for comparison.
3. Compare `stat -c %s` of the rebuilt image with the original partition size.
4. `e2fsck -fn` on Linux (container) — exit 0 required.
5. `aimg ext4 extract` back and `cmp` a large APK, an ELF and `build.prop`.
6. For super builds: `aimg super dump` and compare partition sizes/order/extents with the
   original.

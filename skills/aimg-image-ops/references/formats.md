# Formats and layout notes

## Sparse image

- Header 28 bytes: magic `0xED26FF3A`, major 1, file_hdr_sz 28, chunk_hdr_sz 12,
  blk_sz, total_blks, total_chunks, checksum.
- Chunk header 12 bytes: type, reserved, chunk_sz, total_sz.
- Types: `0xCAC1` RAW, `0xCAC2` FILL, `0xCAC3` DONT_CARE, `0xCAC4` CRC32.
- `to-raw` concatenates multiple sparse inputs.

## super (LP metadata)

Standard layout, in bytes from the start of the super partition:

```
0        .. 4096      reserved (LP_PARTITION_RESERVED_BYTES)
4096     .. 8192      primary geometry (magic 0x616C4467)
8192     .. 12288     backup geometry
12288    .. +max*slots primary metadata slots
...+max*slots .. +2*max*slots  backup metadata slots
```

Metadata header magic `0x414C5030`, major 10. Tables hold partitions (52-byte entries),
extents (24), groups (48), block devices (60). `super make` writes geometry at 4096/8192 and
places data at `align_up(4096 + 8192 + 2*max*slots, alignment)`.

A logical partition extent is `num_sectors` at `target_data` (sectors) inside the super
partition; `target_type` 0 = linear, 1 = zero.

## boot / vendor_boot headers

- boot v0/v1/v2: `ANDROID!`, page_size at 36 (power of two), header sizes 1632/1648/1660,
  `id[8]` at 576 holds SHA1(kernel|ramdisk|second[|recovery_dtbo][|dtb]).
- boot v3/v4: page_size implicitly 4096, header 1580/1584, cmdline 44..1580, signature_size
  at 1580 for v4.
- vendor_boot: `VNDRBOOT`, header 2112 (v3) or 2128 (v4); v4 has a 108-byte ramdisk table
  (size, offset, type, name[32], board_id[16]); fragments are concatenated in table order.

## payload.bin

Header: magic `CrAU`, version u64 BE, manifest size u64 BE, (v2+) metadata signature size u32.
Manifest is protobuf `update_metadata.proto`. Install operations:

```
0 REPLACE, 1 REPLACE_BZ, 4 ZERO, 5 DISCARD, 6 REPLACE_XZ,
7 SOURCE_COPY, 8 SOURCE_BSDIFF
```

`data_sha256` covers the stored blob (compressed bytes when the op is BZ/XZ), which is what
payload-dumper and update_engine verify before decompressing. `new_info.hash` is the whole
partition SHA256.

## transfer.list

Header: version, total_blocks, (v2+) two stash lines. Commands:

```
new <rangeset>
zero <rangeset>
erase <rangeset>
copy <src_rs> <dst_rs>
move <src_rs> <dst_rs>
bsdiff <patch_off> <patch_len> [src_hash dst_hash] <src_rs> <dst_rs>
stash <id> [rangeset]
free <id>
```

`rangeset` is `count,begin,end,begin,end,...` where `count` equals the number of following
integers. `dat merge` output size is `max(total_blocks, last command block)`.

## meta.txt

TSV, header `# aimg-meta v1`, 11 columns:

```
kind  path  uid  gid  mode  context  caps  size  mtime  link_target  ino
```

- `kind`: `D` directory, `F` file, `L` symlink.
- `mode`: octal (`0755`). `caps`: hex (`0x40`) → `security.capability` (vfs_cap_data rev2).
- `ino` is optional; rows sharing it become hard links to one inode on rebuild.
- `security.selinux` comes from `context`; values are stored as `context\0`.

## ext4 builder subset

Features written: `extents`, `filetype`, `sparse_super`, `large_file`, `ext_attr`.
Not written: `metadata_csum`, `64bit`, `htree`, inline data. Inode size defaults to 256 and
xattrs are written inline when they fit (96-byte body area), otherwise one external xattr
block per inode. Block groups reserve `SB + backup GDT` before bitmaps; 1 KiB group 0 accounts
for `first_data_block`; group size is capped so one group stays below 4 GiB.

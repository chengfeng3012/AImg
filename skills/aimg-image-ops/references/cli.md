# aimg CLI reference

`aimg <command> [subcommand] [flags]`. `--help` on any level prints the authoritative flags.
Paths below are examples.

## sparse

```
aimg sparse to-raw --out RAW INPUT...
aimg sparse from-raw [--block-size 4096] --out OUT SPARSE
aimg sparse info SPARSE
```

- `to-raw` accepts multiple inputs; they are concatenated (sparsechunk use case).
- `from-raw` emits `CHUNK_TYPE_RAW` / `FILL` / `DONT_CARE` chunks with 4096-byte block size by
  default.

## super

```
aimg super dump [--slot 0] [--json] SUPER
aimg super unpack [--slot 0] [--keep-suffix] [--partitions system,vendor]
                  [--sparse] [--sparse-block 4096] --out DIR SUPER
aimg super make --out SUPER --partition name=IMAGE[:group] ...
                [--size BYTES] [--metadata-max-size 65536] [--metadata-slots 2]
                [--alignment 1048576] [--minor-version 2] [--sparse] [--sparse-block 4096]
```

- `--slot` selects the LP metadata copy **and** filters `_b` (slot 0) / `_a` (slot 1).
- `--partitions` matches base names (`system` matches `system_a`/`system_b`) or exact names.
- `super make` refuses sparse partition images; expand them with `sparse to-raw` first.
- Partition image sizes must match the sizes recorded in the original metadata.

## boot

```
aimg boot info [--json] IMAGE
aimg boot unpack --out DIR IMAGE
aimg boot pack --out IMAGE DIR
```

`unpack` writes `boot.json` plus `kernel`, `ramdisk`, `second`, `recovery_dtbo`, `dtb`,
`boot_signature`, `bootconfig`, and for vendor_boot v4 `vendor_ramdisk` and
`vendor_ramdisk.<name>` fragments.

## payload

```
aimg payload info [--json] PAYLOAD
aimg payload extract [--partitions a,b] [--old BASE_DIR] [--verify [true|false]] --out DIR PAYLOAD
aimg payload make --out PAYLOAD --partition name=IMAGE ...
                  [--chunk-size 2097152] [--compress] [--key key.pem] [--max-timestamp 0]
```

- `--old` is required only when the payload actually contains `SOURCE_COPY`/`SOURCE_BSDIFF`.
- `make` writes `<out>.properties` with `FILE_HASH`, `FILE_SIZE`, `METADATA_HASH`,
  `METADATA_SIZE`.

## dat

```
aimg dat merge --out IMG TRANSFER_LIST NEW_DAT
aimg dat split [--prefix system] [--version 4] --out DIR RAW_IMG
aimg dat patch --out IMG BASE_IMG TRANSFER_LIST NEW_DAT PATCH_DAT
aimg dat br --out DAT FILE.dat.br
aimg dat info [--json] TRANSFER_LIST
```

`transfer.list` commands understood: `new`, `zero`, `erase`, `copy`, `move`,
`bsdiff <patch_off> <patch_len> [hash...] <src_rs> <dst_rs>`, `stash <id> [ranges]`,
`free <id>`. Unknown commands are an error.

## ext4

```
aimg ext4 info [--json] IMG
aimg ext4 ls [--limit 500] IMG
aimg ext4 extract [--meta META] [--file-contexts FILE_CONTEXTS] --out DIR IMG
aimg ext4 build --dir DIR --out IMG [--meta META] [--fs-config FS_CONFIG]
                [--file-contexts FILE_CONTEXTS] [--size BYTES]
                [--block-size 4096] [--label aimg]
```

- `extract` writes `meta.txt` into the output directory unless `--meta` says otherwise.
- `build` without `--meta` scans the tree and resolves permissions from the text configs;
  files not listed in a supplied `--meta` are skipped with a warning.
- Input images may be sparse; `info`/`ls`/`extract` expand them transparently.

## perm

```
aimg perm export --dir DIR --out META [--fs-config FSC] [--file-contexts FC]
aimg perm apply --meta META --dir DIR
aimg perm show --meta META PATH
```

`perm apply` only sets read-only bits and mtimes on the host; uid/gid/SELinux/caps stay in
`meta.txt` and are applied by `ext4 build`.

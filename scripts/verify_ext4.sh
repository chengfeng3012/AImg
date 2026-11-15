#!/bin/bash
# End-to-end verification of the ext4 writer against real e2fsprogs.
#   scripts/verify_ext4.sh [host_dir]
# Produces <host_dir>/.verify/system.img and runs e2fsck -fn on it.
set -e
HOST_DIR="${1:-$(pwd)}"
V="$HOST_DIR/.verify"

rm -rf "$V"
mkdir -p "$V/tree/system/bin" "$V/tree/system/etc" "$V/tree/system/app/Foo"
printf 'ro.product.name=aimg\n' > "$V/tree/system/etc/build.prop"
head -c 200000 /dev/urandom > "$V/tree/system/bin/blob"
head -c 50000 /dev/urandom > "$V/tree/system/app/Foo/Foo.apk"
ln -s /system/bin/blob "$V/tree/system/bin/blob_link"
cp "$HOST_DIR/configs/fs_config.txt" "$V/"
cp "$HOST_DIR/configs/file_contexts" "$V/"

docker run --rm -v "$HOST_DIR":/w -w /w rust:1-slim bash -c '
  export PATH=/usr/local/cargo/bin:$PATH
  cargo build --release >/dev/null
  ./target/release/aimg perm export --dir /w/.verify/tree --out /w/.verify/meta.txt \
    --fs-config /w/.verify/fs_config.txt --file-contexts /w/.verify/file_contexts
  ./target/release/aimg ext4 build --dir /w/.verify/tree --out /w/.verify/system.img \
    --meta /w/.verify/meta.txt --size 64M --label system
  ./target/release/aimg ext4 extract /w/.verify/system.img --out /w/.verify/roundtrip
'

docker run --rm -v "$V":/v debian:stable-slim bash -c '
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq e2fsprogs >/dev/null 2>&1
  e2fsck -fn /v/system.img
  echo "e2fsck_exit=$?"
  debugfs -R "ea_list /system/bin/blob" /v/system.img 2>/dev/null
'

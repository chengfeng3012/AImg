#!/bin/bash
# probe: ext4 builder group layout / block-size 1024 / size-at-group-boundary
BIN=/root/AndroidImg_Final/target/release/aimg
W=/tmp/aimgv
rm -rf $W
mkdir -p $W/tree/system/bin $W/tree/system/etc
printf 'ro.x=1\n' > $W/tree/system/etc/build.prop
head -c 200000 /dev/urandom > $W/tree/system/bin/blob
ln -s /system/bin/blob $W/tree/system/bin/blob_link
$BIN perm export --dir $W/tree --out $W/meta.txt || echo EXPORT_FAIL
wc -l $W/meta.txt

echo "=== 32M control (1 group) ==="
$BIN ext4 build --dir $W/tree --meta $W/meta.txt --out $W/s32.img --size 32M --label system
e2fsck -fn $W/s32.img 2>&1 | tail -20
echo "exit32=${PIPESTATUS[0]}"

echo "=== 256M (groups>1) ==="
$BIN ext4 build --dir $W/tree --meta $W/meta.txt --out $W/s256.img --size 256M --label system
e2fsck -fn $W/s256.img 2>&1 | head -60
echo "exit256=$?"
dumpe2fs -h $W/s256.img 2>/dev/null | head -35
echo "--- group 1 layout ---"
dumpe2fs $W/s256.img 2>/dev/null | sed -n '/^Group 1:/,/^Group 2:/p'

echo "=== 130M (just past a group boundary) ==="
$BIN ext4 build --dir $W/tree --meta $W/meta.txt --out $W/s130.img --size 130M --label system
echo "exit130=$?"

echo "=== 1024 block size ==="
$BIN ext4 build --dir $W/tree --meta $W/meta.txt --out $W/b1k.img --size 32M --block-size 1024 --label system
echo "buildb1k=$?"
e2fsck -fn $W/b1k.img 2>&1 | head -40
echo "fsckb1k=$?"

echo "=== extract roundtrip from 256M ==="
$BIN ext4 extract $W/s256.img --out $W/rt 2>&1 | tail -5
ls -l $W/rt/system/bin 2>/dev/null
cmp $W/tree/system/bin/blob $W/rt/system/bin/blob && echo BLOB_OK || echo BLOB_MISMATCH

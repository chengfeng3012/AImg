#!/bin/bash
# probe 5: implicit-parent flattening, truncated boot/vendor_boot panic, 2-partition size overrun
BIN=/root/AndroidImg_Final/target/release/aimg
W=/tmp/aimgv5
rm -rf $W; mkdir -p $W/tree/a/b
printf 'deep\n' > $W/tree/a/b/c.txt
echo "=== meta with only the deep file (no /a, /a/b dirs) ==="
printf '# aimg-meta v1\nF\t/a/b/c.txt\t0\t0\t0644\tu:object_r:system_file:s0\t0x0\t5\t1700000000\t\n' > $W/meta.txt
$BIN ext4 build --dir $W/tree --meta $W/meta.txt --out $W/deep.img --size 32M --label t >/dev/null && echo BUILD_OK
echo "--- ext4 ls ---"
$BIN ext4 ls $W/deep.img
echo "--- debugfs root dir + /a dir ---"
debugfs -R "ls -l /" $W/deep.img 2>/dev/null
debugfs -R "ls -l /a" $W/deep.img 2>/dev/null
debugfs -R "ls -l /a/b" $W/deep.img 2>/dev/null
e2fsck -fn $W/deep.img 2>&1 | tail -3

echo "=== truncated boot.img (100 bytes, magic only) ==="
head -c 100 /dev/zero > $W/trunc.img
printf 'ANDROID!' | dd of=$W/trunc.img bs=1 seek=0 conv=notrunc status=none
$BIN boot info $W/trunc.img; echo "exit_boot_trunc=$?"

echo "=== truncated vendor_boot (100 bytes) ==="
head -c 100 /dev/zero > $W/truncv.img
printf 'VNDRBOOT' | dd of=$W/truncv.img bs=1 seek=0 conv=notrunc status=none
$BIN boot unpack $W/truncv.img --out $W/tv; echo "exit_vb_trunc=$?"

echo "=== boot v0 with bad page size / empty file ==="
: > $W/empty.img
$BIN boot info $W/empty.img; echo "exit_empty=$?"

echo "=== super make: image bigger than declared size? (library API only, skip) ==="
echo "=== payload make --compress: op types + data_length ==="
head -c 300000 /dev/urandom > $W/p.img
$BIN payload make --out $W/pc.bin --partition system=$W/p.img --compress --chunk-size 1048576 >/dev/null
$BIN payload info --json $W/pc.bin | head -60

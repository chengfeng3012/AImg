#!/bin/bash
# probe 3: read real mke2fs images (ext4 extents+metadata_csum / ext2 indirect blocks / htree dir)
BIN=/root/AndroidImg_Final/target/release/aimg
W=/tmp/aimgv3
rm -rf $W; mkdir -p $W/tree/sub
head -c 4096 /dev/urandom > $W/tree/small.bin
head -c 300000 /dev/urandom > $W/tree/big.bin
head -c 5000 /dev/urandom > $W/tree/sub/nested.bin
for i in $(seq 1 300); do head -c 10 /dev/urandom > $W/tree/f_$i.dat; done
mkfs.ext4 -q -F -d $W/tree $W/e4.img 32M 2>&1 | tail -2
echo "=== ext4 (extents, metadata_csum, 64bit) ==="
$BIN ext4 info $W/e4.img
$BIN ext4 extract $W/e4.img --out $W/o4 2>&1 | tail -3
cmp $W/tree/big.bin $W/o4/big.bin && echo BIG_OK || echo BIG_MISMATCH
cmp $W/tree/small.bin $W/o4/small.bin && echo SMALL_OK || echo SMALL_MISMATCH
echo "count f_*.dat extracted:"; ls $W/o4 | grep -c '^f_'

echo "=== ext2 (no extents -> indirect blocks) ==="
mkfs.ext2 -q -F -d $W/tree $W/e2.img 32M 2>&1 | tail -2
$BIN ext4 info $W/e2.img
$BIN ext4 extract $W/e2.img --out $W/o2 2>&1 | tail -3
cmp $W/tree/big.bin $W/o2/big.bin && echo E2_BIG_OK || echo E2_BIG_MISMATCH
echo "e2 big.bin size extracted:"; stat -c %s $W/o2/big.bin 2>/dev/null
echo "e2 small.bin:"; cmp $W/tree/small.bin $W/o2/small.bin && echo E2_SMALL_OK || echo E2_SMALL_MISMATCH

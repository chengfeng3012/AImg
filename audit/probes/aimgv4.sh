#!/bin/bash
# probe 4: control fsck, xattr ibody read (debugfs ea_set), tool-built image ea_list
BIN=/root/AndroidImg_Final/target/release/aimg
W=/tmp/aimgv4
rm -rf $W; mkdir -p $W

echo "=== control: 32M (1 group) e2fsck ==="
e2fsck -fn /tmp/aimgv/s32.img 2>&1 | tail -4
echo "exit32=$?"

echo "=== 256M (2 groups) e2fsck head ==="
e2fsck -fn /tmp/aimgv/s256.img 2>&1 | head -10

echo "=== ibody xattr read test (debugfs ea_set on a real mke2fs ext4) ==="
cp /tmp/aimgv3/e4.img $W/x.img
debugfs -w -R "ea_set /big.bin security.selinux u:object_r:test_ibody:s0" $W/x.img 2>&1 | tail -2
debugfs -R "ea_list /big.bin" $W/x.img 2>&1 | tail -3
$BIN ext4 extract $W/x.img --out $W/o >/dev/null 2>&1
grep 'big.bin' $W/o/meta.txt
echo "--- inline (ibody) vs external block: check inode i_file_acl ---"
debugfs -R "stat /big.bin" $W/x.img 2>&1 | grep -Ei "file acl|xattr|Blockcount|Size"

echo "=== tool-built image ea_list (external xattr block) ==="
debugfs -R "ea_list /system/bin/blob" /tmp/aimgv/s32.img 2>&1 | tail -3
echo "=== tool-built image: i_blocks vs e2fsck already checked above ==="

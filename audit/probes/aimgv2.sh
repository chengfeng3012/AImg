#!/bin/bash
# probe 2: super unpack duplicate names + boot v2/v4 header_size fields
BIN=/root/AndroidImg_Final/target/release/aimg
W=/tmp/aimgv2
rm -rf $W; mkdir -p $W

head -c 100000 /dev/urandom > $W/a.img
head -c 120000 /dev/urandom > $W/b.img
head -c 10000 /dev/urandom > $W/c.img

echo "=== super make with system_a + system_b ==="
$BIN super make --out $W/super.img --partition system_a=$W/a.img --partition system_b=$W/b.img --size 64M >/dev/null
$BIN super dump $W/super.img | head -20

echo "--- unpack slot 0, default flags ---"
$BIN super unpack $W/super.img --out $W/out0 --slot 0
ls -l $W/out0
echo "a.img size:"; stat -c %s $W/a.img; echo "b.img size:"; stat -c %s $W/b.img
echo "out0/system.img size:"; stat -c %s $W/out0/system.img
echo "compare with a:"; cmp -s <(head -c 100000 $W/out0/system.img) $W/a.img && echo MATCH_A || echo NOT_A
echo "compare with b:"; cmp -s <(head -c 120000 $W/out0/system.img) $W/b.img && echo MATCH_B || echo NOT_B

echo "--- unpack slot 0, keep-suffix ---"
$BIN super unpack $W/super.img --out $W/out0k --slot 0 --keep-suffix
ls -l $W/out0k

echo "--- unpack slot 1, default ---"
$BIN super unpack $W/super.img --out $W/out1 --slot 1
ls -l $W/out1

echo "=== boot v4 header fields ==="
mkdir -p $W/b4
cat > $W/b4/boot.json <<'EOF'
{"kind":"boot","header_version":4,"page_size":4096,"kernel_addr":32768,"ramdisk_addr":285212672,"second_addr":284164096,"tags_addr":268435712,"dtb_addr":301989888,"recovery_dtbo_offset":0,"name":"","cmdline":"console=ttyMSM0","extra_cmdline":"","os_version":0,"signature_size":0,"avb":null}
EOF
head -c 5000 /dev/urandom > $W/b4/kernel
head -c 3000 /dev/urandom > $W/b4/ramdisk
head -c 64  /dev/urandom > $W/b4/boot_signature
$BIN boot pack $W/b4 --out $W/bootv4.img
echo "header_size@20 (expect 1584):"; od -An -tu4 -j20 -N4 $W/bootv4.img
echo "sig_size@1580 (expect 64):";   od -An -tu4 -j1580 -N4 $W/bootv4.img
od -An -c -j0 -N8 $W/bootv4.img

echo "=== boot v2 header fields ==="
mkdir -p $W/b2
cat > $W/b2/boot.json <<'EOF'
{"kind":"boot","header_version":2,"page_size":2048,"kernel_addr":32768,"ramdisk_addr":285212672,"second_addr":284164096,"tags_addr":268435712,"dtb_addr":301989888,"recovery_dtbo_offset":0,"name":"test","cmdline":"console=ttyMSM0","extra_cmdline":"foo=1","os_version":0,"signature_size":0,"avb":null}
EOF
head -c 5000 /dev/urandom > $W/b2/kernel
head -c 3000 /dev/urandom > $W/b2/ramdisk
head -c 700  /dev/urandom > $W/b2/dtb
$BIN boot pack $W/b2 --out $W/bootv2.img
echo "v2 header_size@1644 (expect 1660):"; od -An -tu4 -j1644 -N4 $W/bootv2.img
echo "v2 recovery_dtbo_size@1632:"; od -An -tu4 -j1632 -N4 $W/bootv2.img
echo "v2 dtb_size@1648:"; od -An -tu4 -j1648 -N4 $W/bootv2.img
$BIN boot info $W/bootv2.img
$BIN boot info $W/bootv4.img

echo "=== vendor_boot v4 fragment edit semantics ==="
mkdir -p $W/vb
cat > $W/vb/boot.json <<'EOF'
{"kind":"vendor_boot","header_version":4,"page_size":4096,"kernel_addr":0,"ramdisk_addr":0,"second_addr":0,"tags_addr":0,"dtb_addr":0,"recovery_dtbo_offset":0,"name":"","cmdline":"","extra_cmdline":"","os_version":0,"signature_size":0,"avb":null,"vendor_ramdisks":[{"name":"platform","ramdisk_type":1,"size":100,"offset":0},{"name":"dlkm","ramdisk_type":3,"size":50,"offset":100}]}
EOF
head -c 100 /dev/urandom > $W/vb/vendor_ramdisk.platform
head -c 50  /dev/urandom > $W/vb/vendor_ramdisk.dlkm
head -c 300 /dev/urandom > $W/vb/dtb
printf 'androidboot.foo=bar\n' > $W/vb/bootconfig
python3 - "$W" <<'PY'
import sys, os
W = sys.argv[1]
p1 = open(os.path.join(W,'vb','vendor_ramdisk.platform'),'rb').read()
p2 = open(os.path.join(W,'vb','vendor_ramdisk.dlkm'),'rb').read()
open(os.path.join(W,'vb','vendor_ramdisk'),'wb').write(p1+p2)
PY
$BIN boot pack $W/vb --out $W/vb.img && echo VB_PACK_OK
$BIN boot info $W/vb.img
echo "--- now EDIT one fragment (append 10 bytes) and repack ---"
head -c 110 /dev/urandom > $W/vb/vendor_ramdisk.platform
$BIN boot pack $W/vb --out $W/vb2.img && echo VB2_PACK_OK
$BIN boot unpack $W/vb2.img --out $W/vb_check >/dev/null
ls -l $W/vb_check
echo "platform size after roundtrip (expect 110):"; stat -c %s $W/vb_check/vendor_ramdisk.platform 2>/dev/null

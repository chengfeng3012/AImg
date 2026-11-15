#!/bin/bash
# probe 6: crafted truncated headers (valid page_size) -> panic?  + payload xz hash mismatch
BIN=/root/AndroidImg_Final/target/release/aimg
W=/tmp/aimgv6
rm -rf $W; mkdir -p $W

echo "=== boot.img: 1000 bytes, valid page_size 2048, v0 ==="
head -c 1000 /dev/zero > $W/t1.img
printf 'ANDROID!' | dd of=$W/t1.img bs=1 seek=0 conv=notrunc status=none
printf '\x00\x08\x00\x00' | dd of=$W/t1.img bs=1 seek=36 conv=notrunc status=none
$BIN boot info $W/t1.img; echo "exit=$?"

echo "=== vendor_boot: 1000 bytes, valid page_size 4096 ==="
head -c 1000 /dev/zero > $W/t2.img
printf 'VNDRBOOT' | dd of=$W/t2.img bs=1 seek=0 conv=notrunc status=none
printf '\x00\x10\x00\x00' | dd of=$W/t2.img bs=1 seek=12 conv=notrunc status=none
$BIN boot info $W/t2.img; echo "exit=$?"

echo "=== vendor_boot v4 1500 bytes (valid page_size, truncated table) ==="
head -c 1500 /dev/zero > $W/t3.img
printf 'VNDRBOOT' | dd of=$W/t3.img bs=1 seek=0 conv=notrunc status=none
printf '\x00\x10\x00\x00' | dd of=$W/t3.img bs=1 seek=12 conv=notrunc status=none
printf '\x04\x00\x00\x00' | dd of=$W/t3.img bs=1 seek=8 conv=notrunc status=none
$BIN boot info $W/t3.img; echo "exit=$?"

echo "=== payload --compress with compressible data (xz path) ==="
python3 -c "open('$W/p.img','wb').write(b'Android payload test line\n'*12000)"
$BIN payload make --out $W/pc.bin --partition system=$W/p.img --compress >/dev/null
$BIN payload info --json $W/pc.bin > $W/pj.json
python3 - "$W" <<'PY'
import sys, json, hashlib
W = sys.argv[1]
j = json.load(open(W+'/pj.json'))
print("ops:", [(o['data_length'], o.get('dst_length')) for o in j['partitions'][0]['ops']])
print("new_info.size:", j['partitions'][0]['new_info']['size'])
data_start = 24 + j['manifest_size'] + j['metadata_signature_size']
buf = open(W+'/pc.bin','rb').read()
img = open(W+'/p.img','rb').read()
for o in j['partitions'][0]['ops']:
    blob = buf[data_start+o['data_offset']: data_start+o['data_offset']+o['data_length']]
    h_blob = hashlib.sha256(blob).hexdigest()
    h_claim = bytes(o['data_sha256']).hex()
    print("op data_offset=%d data_length=%d" % (o['data_offset'], o['data_length']))
    print("  sha256(stored blob) =", h_blob)
    print("  data_sha256 in proto =", h_claim)
    print("  MATCH" if h_blob == h_claim else "  *** MISMATCH ***")
    # is it the hash of the uncompressed chunk?
    chunk = img[o['dst_extents'][0]['start_block']*4096: o['dst_extents'][0]['start_block']*4096+o['dst_length']]
    print("  sha256(uncompressed chunk) =", hashlib.sha256(chunk).hexdigest())
PY

echo "=== extract with --verify (own tool) ==="
$BIN payload extract $W/pc.bin --out $W/ex >/dev/null && cmp $W/p.img $W/ex/system.img && echo XZ_ROUNDTRIP_OK || echo XZ_ROUNDTRIP_BAD

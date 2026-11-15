#!/usr/bin/env bash
# Verify an ext4 image with an independent checker plus a full read-back.
# usage: verify_image.sh IMAGE [WORKDIR]
set -euo pipefail

img=${1:?usage: verify_image.sh IMAGE [WORKDIR]}
work=${2:-$(mktemp -d)}

echo "== e2fsck =="
e2fsck -fn "$img"

echo "== aimg info =="
aimg ext4 info "$img"

echo "== read-back =="
aimg ext4 extract "$img" --out "$work"
find "$work" -type f | wc -l
echo "extracted to $work"

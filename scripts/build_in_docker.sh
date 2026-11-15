#!/bin/bash
# Build + test this crate inside Docker (never on the Windows host).
# Usage: scripts/build_in_docker.sh [host_dir] [extra cargo args...]
set -e
HOST_DIR="${1:-$(pwd)}"
shift || true
docker run --rm \
  -e CARGO_BUILD_JOBS=2 \
  -v "$HOST_DIR":/w -w /w \
  rust:1-slim \
  /bin/bash -c "export PATH=/usr/local/cargo/bin:\$PATH; cargo test $*"

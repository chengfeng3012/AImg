#!/bin/bash
# Cross-compile the Windows binary on a Linux host inside Docker.
# Usage: scripts/build_windows_in_docker.sh [host_dir]
set -e
HOST_DIR="${1:-$(pwd)}"
docker run --rm -v "$HOST_DIR":/w -w /w rust:1-slim bash -c '
  set -e
  apt-get update -qq
  apt-get install -y -qq gcc-mingw-w64-x86-64 >/dev/null
  rustup target add x86_64-pc-windows-gnu >/dev/null
  export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
  export CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++
  export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
  export CARGO_BUILD_JOBS=2
  cargo build --release --target x86_64-pc-windows-gnu
'
ls -l "$HOST_DIR/target/x86_64-pc-windows-gnu/release/aimg.exe"

#!/usr/bin/env bash
# Cross-compile tuxtop-watch.exe for Windows from Linux or WSL.
#
# Lets you get a Windows binary without installing Rust and the MSVC build
# tools on the Windows side. Only works for the CLI: the Tauri GUI still has
# to be built on Windows (ADR-006).
#
# One-time setup:
#   sudo apt install mingw-w64
#   rustup target add x86_64-pc-windows-gnu
set -euo pipefail

TARGET=x86_64-pc-windows-gnu
OUT="target/$TARGET/release/tuxtop-watch.exe"

if ! command -v x86_64-w64-mingw32-gcc >/dev/null; then
  echo "error: mingw-w64 not installed. run: sudo apt install mingw-w64" >&2
  exit 1
fi

if ! rustup target list --installed | grep -qx "$TARGET"; then
  echo "error: rust target missing. run: rustup target add $TARGET" >&2
  exit 1
fi

cargo build --release --target "$TARGET" --bin tuxtop-watch
echo
echo "built: $OUT"

# When run from WSL, drop it somewhere Windows can see if a destination is given.
if [ "${1:-}" != "" ]; then
  cp "$OUT" "$1"
  echo "copied to: $1"
fi

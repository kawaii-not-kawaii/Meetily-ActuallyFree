#!/usr/bin/env bash
# Source this (". scripts/linux-sysroot-env.sh") to build the Tauri crate on a
# Linux box without system -dev packages. Expects the user-space sysroot
# (extracted .debs) at $MEETILY_SYSROOT/root, default ~/.local/meetily-sysroot.
# Read-only: it never writes to the sysroot. Set CARGO_TARGET_DIR yourself if
# you do not want ./target.
#
# Also stages the two gitignored build inputs the Tauri build needs:
#   frontend/src-tauri/binaries/llama-helper-<triple>  (built from llama-helper/)
#   frontend/out/index.html                            (stub if no Next build)
_sr="${MEETILY_SYSROOT:-$HOME/.local/meetily-sysroot}/root"
_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
_triple="x86_64-unknown-linux-gnu"
_lib="$_sr/usr/lib/x86_64-linux-gnu"

export PATH="$PATH:$_sr/usr/bin"   # pkgconf, clang, ... (appended: system tools win)
export PKG_CONFIG="$_sr/usr/bin/pkgconf"
export PKG_CONFIG_SYSROOT_DIR="$_sr"
export PKG_CONFIG_PATH="$_lib/pkgconfig:$_sr/usr/share/pkgconfig"
export PKG_CONFIG_ALLOW_CROSS=1
export LIBRARY_PATH="$_lib${LIBRARY_PATH:+:$LIBRARY_PATH}"
export LD_LIBRARY_PATH="$_lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export CPATH="$_sr/usr/include:$_sr/usr/include/x86_64-linux-gnu${CPATH:+:$CPATH}"
export RUSTFLAGS="-L native=$_lib ${RUSTFLAGS:-}"
export BINDGEN_EXTRA_CLANG_ARGS="-idirafter $_sr/usr/include -idirafter $_sr/usr/include/x86_64-linux-gnu ${BINDGEN_EXTRA_CLANG_ARGS:-}"

mkdir -p "$_root/frontend/out" "$_root/frontend/src-tauri/binaries"
[ -e "$_root/frontend/out/index.html" ] || echo '<!doctype html><title>stub</title>' > "$_root/frontend/out/index.html"
_helper="$_root/frontend/src-tauri/binaries/llama-helper-$_triple"
if [ ! -e "$_helper" ]; then
  # Use a placeholder (enough for tauri-build's existence check); build the real
  # one with `cargo build -p llama-helper --release` and copy it if you need to run.
  printf '#!/bin/sh\nexit 1\n' > "$_helper"; chmod +x "$_helper"
fi
unset _sr _root _triple _lib _helper

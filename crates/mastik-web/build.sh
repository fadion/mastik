#!/usr/bin/env bash
# Builds the prototype player that `mastik export --prototype` packs: the wasm and
# its loader script, written to the folder given (default `target/player`, where
# a `cargo run` build of the app looks). `build.sh --check` only checks that
# both players compile, writing no player and needing no wasm-bindgen: CI's gate.
#
# Needs rustup (this folder's `rust-toolchain.toml` installs its nightly) and
# `cargo install wasm-bindgen-cli --version 0.2.108 --locked`, the version
# `Cargo.toml` pins.
set -euo pipefail
check=false
if [ "${1:-}" = --check ]; then
  check=true
  shift
fi
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
out="${1:-$root/target/player}"
target="${CARGO_TARGET_DIR:-$root/target/web}" # `.cargo/config.toml`'s default
# Every path the caller gives is read from the caller's folder, so made whole
# before the `cd` below.
mkdir -p "$target"
target="$(cd "$target" && pwd)"
if [ -n "${WASI_SDK:-}" ]; then WASI_SDK="$(cd "$WASI_SDK" && pwd)"; fi
if $check; then
  run=(check --locked)
else
  run=(build --release --locked)
  mkdir -p "$out"
  out="$(cd "$out" && pwd)"
  want="wasm-bindgen 0.2.108"
  have="$(wasm-bindgen --version 2>/dev/null || true)"
  if [ "$have" != "$want" ]; then
    echo "build.sh: needs $want (found: ${have:-none}); cargo install wasm-bindgen-cli --version 0.2.108 --locked" >&2
    exit 1
  fi
fi
cd "$here"
CARGO_TARGET_DIR="$target" cargo "${run[@]}"
if $check; then
  echo "the player compiles"
else
  wasm-bindgen --target no-modules --no-typescript --out-dir "$out" \
    "$target/wasm32-unknown-unknown/release/mastik_web.wasm"
  echo "wrote $out/mastik_web_bg.wasm and $out/mastik_web.js"
fi

# With `WASI_SDK` naming an unpacked wasi-sdk, the player that runs scripts too,
# into `<out>-scripts`: Luau's C++ built by wasi-sdk's clang against its wasip1
# sysroot, linked with its exception-handling libc++ (`SPIKE.md` says why each
# setting is there). An export packs it only when its prototype runs a script.
if [ -n "${WASI_SDK:-}" ]; then
  s="$WASI_SDK/share/wasi-sysroot"
  shim="$target-scripts/shim"
  mkdir -p "$shim"
  ln -sf "$s/lib/wasm32-wasip1/eh/libc++.a" "$shim/libstdc++.a"
  flags="--target=wasm32-wasip1 --sysroot=$s -fwasm-exceptions -mllvm -wasm-use-legacy-eh=false -include stdlib.h"
  CC_wasm32_unknown_unknown="$WASI_SDK/bin/clang" \
  CXX_wasm32_unknown_unknown="$WASI_SDK/bin/clang++" \
  AR_wasm32_unknown_unknown="$WASI_SDK/bin/llvm-ar" \
  CXXSTDLIB_wasm32_unknown_unknown=c++ \
  CFLAGS_wasm32_unknown_unknown="$flags" \
  CXXFLAGS_wasm32_unknown_unknown="$flags" \
  CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="-C panic=unwind -C target-feature=+exception-handling -L $shim -L $s/lib/wasm32-wasip1/eh -L $s/lib/wasm32-wasip1 -l static=c++ -l static=c++abi -l static=unwind -l static=wasi-emulated-process-clocks -l static=c -C link-arg=--allow-multiple-definition" \
  CARGO_TARGET_DIR="$target-scripts" cargo "${run[@]}" --features scripts
  if $check; then
    echo "the scripts player compiles"
  else
    wasm-bindgen --target no-modules --no-typescript --out-dir "$out-scripts" \
      "$target-scripts/wasm32-unknown-unknown/release/mastik_web.wasm"
    echo "wrote $out-scripts/mastik_web_bg.wasm and $out-scripts/mastik_web.js"
  fi
elif $check; then
  echo "the scripts player is not checked: set WASI_SDK to an unpacked wasi-sdk" >&2
fi

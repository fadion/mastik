#!/usr/bin/env bash
# Builds the prototype player that `mastik export --prototype` packs: the wasm and
# its loader script, written to the folder given (default `target/player`, where
# a `cargo run` build of the app looks).
#
# Needs rustup (this folder's `rust-toolchain.toml` installs its nightly) and
# `cargo install wasm-bindgen-cli --version 0.2.108 --locked`, the version
# `Cargo.toml` pins.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
out="${1:-$root/target/player}"
target="${CARGO_TARGET_DIR:-$root/target/web}" # `.cargo/config.toml`'s default
want="wasm-bindgen 0.2.108"
have="$(wasm-bindgen --version 2>/dev/null || true)"
if [ "$have" != "$want" ]; then
  echo "build.sh: needs $want (found: ${have:-none}); cargo install wasm-bindgen-cli --version 0.2.108 --locked" >&2
  exit 1
fi
cd "$here"
CARGO_TARGET_DIR="$target" cargo build --release --locked
wasm-bindgen --target no-modules --no-typescript --out-dir "$out" \
  "$target/wasm32-unknown-unknown/release/mastik_web.wasm"
echo "wrote $out/mastik_web_bg.wasm and $out/mastik_web.js"

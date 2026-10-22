#!/usr/bin/env bash
# Builds crates/scrawl-wasm for the browser into packages/scrawl-web/wasm/. Needs the
# wasm32-unknown-unknown target and wasm-bindgen-cli at the lockfile's wasm-bindgen version.
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$root"
cargo build --locked -p scrawl-wasm --release --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir packages/scrawl-web/wasm \
  target/wasm32-unknown-unknown/release/scrawl_wasm.wasm

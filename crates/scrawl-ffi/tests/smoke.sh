#!/usr/bin/env bash
# Compiles tests/smoke.c against the static library `cargo build -p scrawl-ffi` made, and runs it
# under AddressSanitizer, so a leak, double free or bad read across the interface fails it.
#   cargo build -p scrawl-ffi && crates/scrawl-ffi/tests/smoke.sh [target/debug]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
lib="${1:-$here/../../../target/debug}"
out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
cc -std=c11 -g -fsanitize=address -Wall -Wextra -Werror -I"$here/../include" "$here/smoke.c" \
  "$lib/libscrawl_ffi.a" -lpthread -ldl -lm -o "$out/smoke"
ASAN_OPTIONS=detect_leaks=1 "$out/smoke"

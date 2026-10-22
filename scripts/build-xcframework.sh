#!/usr/bin/env bash
# Builds crates/scrawl-ffi into target/ScrawlCore.xcframework: iPhone, a universal iPhone
# simulator slice (Apple silicon and Intel Macs) and a universal Mac slice, each a static
# library with include/scrawl.h as the `ScrawlCore` module.
# ScrawlKit links it when SCRAWL_CORE=1 (Package.swift). Needs Xcode and the Rust targets:
#   rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios aarch64-apple-darwin \
#     x86_64-apple-darwin
set -euo pipefail
cd "$(dirname "$0")/.."
# Match Package.swift's platforms.
export IPHONEOS_DEPLOYMENT_TARGET=18.0 MACOSX_DEPLOYMENT_TARGET=15.0
for target in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios aarch64-apple-darwin \
  x86_64-apple-darwin; do
  cargo build --locked -p scrawl-ffi --release --target "$target"
done
work=target/xcframework
rm -rf "$work" target/ScrawlCore.xcframework
mkdir -p "$work/headers" "$work/ios-sim" "$work/macos"
cp crates/scrawl-ffi/include/scrawl.h "$work/headers/"
cat > "$work/headers/module.modulemap" <<'MODULE'
module ScrawlCore {
  header "scrawl.h"
  export *
}
MODULE
lipo -create target/aarch64-apple-ios-sim/release/libscrawl_ffi.a \
  target/x86_64-apple-ios/release/libscrawl_ffi.a -output "$work/ios-sim/libscrawl_ffi.a"
lipo -create target/aarch64-apple-darwin/release/libscrawl_ffi.a \
  target/x86_64-apple-darwin/release/libscrawl_ffi.a -output "$work/macos/libscrawl_ffi.a"
xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libscrawl_ffi.a -headers "$work/headers" \
  -library "$work/ios-sim/libscrawl_ffi.a" -headers "$work/headers" \
  -library "$work/macos/libscrawl_ffi.a" -headers "$work/headers" \
  -output target/ScrawlCore.xcframework
echo "Built target/ScrawlCore.xcframework"

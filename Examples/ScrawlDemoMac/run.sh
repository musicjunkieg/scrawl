#!/bin/sh
# Builds the demo, wraps it as .build/ScrawlDemo.app with Info.plist (so macOS asks for the
# demo's own screen-recording and microphone permission) and opens it.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
swift build
# A demo still running would keep the old build.
pkill -x ScrawlDemoMac 2>/dev/null && sleep 1 || true
app="$here/.build/ScrawlDemo.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$(swift build --show-bin-path)/ScrawlDemoMac" "$app/Contents/MacOS/"
cp Info.plist "$app/Contents/"
codesign --force --sign - "$app"
open -n "$app"
echo "Opened $app"

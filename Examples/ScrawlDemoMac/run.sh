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
# macOS keeps Screen Recording and Microphone permission for an app's signature. An ad-hoc
# signature changes with every build, so each rebuild would be a new app asking again; a real
# certificate (Xcode's free "Apple Development" one) keeps the permission across rebuilds.
# SCRAWL_SIGN names one; otherwise the first Apple Development identity, else ad hoc.
identity=${SCRAWL_SIGN:-$(security find-identity -v -p codesigning 2>/dev/null |
  sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -n 1)}
if [ -n "$identity" ]; then
  codesign --force --sign "$identity" "$app"
  echo "Signed with $identity"
else
  codesign --force --sign - "$app"
  echo "No Apple Development certificate (Xcode › Settings › Accounts › Manage Certificates):" >&2
  echo "signed ad hoc, so macOS asks for permission again after every rebuild." >&2
fi
open -n "$app"
echo "Opened $app"

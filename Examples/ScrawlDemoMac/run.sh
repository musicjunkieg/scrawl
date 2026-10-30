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
# SCRAWL_SIGN names one; otherwise the first Apple Development identity, by its SHA-1 so two
# certificates with the same name (one renewed, one per Mac) aren't ambiguous; else ad hoc.
identity=${SCRAWL_SIGN:-}
name=$identity
if [ -z "$identity" ]; then
  # A failed lookup isn't "no certificate": stop rather than sign ad hoc and lose the permission.
  if ! ids=$(security find-identity -v -p codesigning); then
    echo "Couldn't list signing certificates; set SCRAWL_SIGN to choose one." >&2
    exit 1
  fi
  found=$(printf '%s\n' "$ids" | grep '"Apple Development:' | head -n 1)
  identity=$(printf '%s\n' "$found" | sed -n 's/^ *[0-9][0-9]*) \([0-9A-F]\{40\}\) .*/\1/p')
  name=$(printf '%s\n' "$found" | sed -n 's/.*"\(.*\)".*/\1/p')
fi
if [ -n "$identity" ]; then
  codesign --force --sign "$identity" "$app"
  echo "Signed with $name"
else
  codesign --force --sign - "$app"
  echo "No Apple Development certificate (Xcode › Settings › Accounts › Manage Certificates):" >&2
  echo "signed ad hoc, so macOS asks for permission again after every rebuild." >&2
fi
open -n "$app"
echo "Opened $app"

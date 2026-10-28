#!/usr/bin/env bash
# Exits non-zero when this Mac's screen must not be used for a real-screen capture.
# Never wakes a display: on Kit's Studio (his desk) it refuses unless he is at it right now,
# i.e. a key or mouse event in the last 60 s. Route captures to the MacBook Pro
# (under /tmp/pro-gui.lock) or a virtual display instead.
set -euo pipefail
host=$(scutil --get LocalHostName 2>/dev/null || hostname)
idle=$(ioreg -c IOHIDSystem -d 4 | awk '/HIDIdleTime/ {print int($NF / 1000000000); exit}')
if [[ $host == *Studio* ]]; then
  if [[ ${SCRAWL_STUDIO_SCREEN_OK:-} != 1 ]]; then
    echo "screen-guard: refusing on $host: run real-screen captures on the Pro (ssh pro, /tmp/pro-gui.lock) or a virtual display" >&2
    exit 1
  fi
  if [[ -z $idle || $idle -ge 60 ]]; then
    echo "screen-guard: refusing on $host: idle ${idle:-?} s, Kit is away or the display is asleep" >&2
    exit 1
  fi
fi

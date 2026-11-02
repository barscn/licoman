#!/bin/bash
# Phase 07 screenshot harness (same flow as capture-p06.sh: float -> resize ->
# move -> focus -> grim; kill stale instances first; title-based targeting).
#
# Captures LIVE application state (no demo fixtures): real discovery, real
# reads. LICOMAN_CAPTURE_NAV navigates after discovery publishes. **Only run
# when the desktop is free** — never while a fullscreen game or the user's
# active work is on screen (Phase 05 lesson).
#
# Usage: tests/tools/capture-p07.sh [shot ...]
#   hyprland   live Desktop → Hyprland: read-only runtime diagnostics rows
#   power      live Shared → Power: honest service-absent states
#   tall-hyprland  same as hyprland on a tall headless output (no scrolling)
MONITOR=DP-5
SHOTDIR=/home/baris/Work/licoman/docs/screenshots
mkdir -p "$SHOTDIR"

kill_licoman() {
  for pid in $(hyprctl -j clients | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if 'licoman' in (c.get('initialClass') or '') or 'licoman' in (c.get('class') or ''):
        print(c.get('pid',''))"); do
    kill -9 "$pid" 2>/dev/null
  done
  sleep 0.5
}

launch_and_capture() {
  local nav="$1" out="$2" sleep_s="$3" title_sub="$4" height="${5:-760}" move_y="${6:-40}"
  kill_licoman
  ( setsid env LICOMAN_CAPTURE_NAV="$nav" /home/baris/Work/licoman/target/release/licoman >/tmp/lc07-shot.log 2>&1 & )
  sleep 2
  local addr=$(hyprctl -j clients | python3 -c "
import json,sys
target='$title_sub'
for c in json.load(sys.stdin):
    if 'licoman' in (c.get('initialClass') or '').lower() and target.lower() in (c.get('title') or '').lower():
        print(c['address']); break
")
  if [ -z "$addr" ]; then
    echo "NO WINDOW for $title_sub ($out)"; kill_licoman; return 1
  fi
  hyprctl -j clients | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if 'licoman' in (c.get('initialClass') or '').lower() and c['address'] != '$addr':
        print(f'minimizewindow address:{c[\"address\"]}')" | while read -r cmd; do hyprctl dispatch "$cmd" >/dev/null; done
  hyprctl dispatch focuswindow "address:$addr" >/dev/null
  hyprctl dispatch togglefloating "address:$addr" >/dev/null
  sleep 0.4
  hyprctl dispatch resizewindowpixel "exact 1160 ${height},address:$addr" >/dev/null
  hyprctl dispatch movewindowpixel "exact 40 ${move_y},address:$addr" >/dev/null
  sleep "$sleep_s"
  local geo=$(hyprctl -j clients | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if c['address'] == '$addr':
        print(f\"{c['at'][0]},{c['at'][1]} {c['size'][0]}x{c['size'][1]}\"); break")
  grim -g "$geo" "$SHOTDIR/$out" && echo "captured $out ($geo)"
}

which="${1:-all}"
case "$which" in
  hyprland)      launch_and_capture "Desktop/Compositor" "p07-live-hyprland.png" 7 "Licoman" ;;
  power)         launch_and_capture "Shared/Power" "p07-live-power.png" 7 "Licoman" ;;
  all)
    launch_and_capture "Desktop/Compositor" "p07-live-hyprland.png" 7 "Licoman"
    launch_and_capture "Shared/Power" "p07-live-power.png" 7 "Licoman"
    ;;
esac
kill_licoman

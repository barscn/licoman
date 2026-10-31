#!/bin/bash
# Phase 05 screenshot harness (Phase 03/04 lessons: float -> resize -> move ->
# focus -> grim; kill stale instances first; title-based targeting).
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
  local demo="$1" out="$2" sleep_s="$3" title_sub="$4"
  kill_licoman
  if [ -n "$demo" ]; then
    ( setsid env LICOMAN_DEMO="$demo" /home/baris/Work/licoman/target/release/licoman >/tmp/lc05-shot.log 2>&1 & )
  else
    ( setsid /home/baris/Work/licoman/target/release/licoman >/tmp/lc05-shot.log 2>&1 & )
  fi
  sleep 2
  # find the window by title substring
  local addr=$(hyprctl -j clients | python3 -c "
import json,sys
target='$title_sub'
for c in json.load(sys.stdin):
    if 'licoman' in (c.get('initialClass') or '').lower() and target.lower() in (c.get('title') or '').lower():
        print(c['address']); break
")
  if [ -z "$addr" ]; then
    echo "NO WINDOW for $title_sub ($out)"; hyprctl -j clients | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if 'licoman' in (c.get('initialClass') or '').lower(): print(c['address'], c.get('title'))"
    kill_licoman; return 1
  fi
  # minimize other licoman windows
  hyprctl -j clients | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if 'licoman' in (c.get('initialClass') or '').lower() and c['address'] != '$addr':
        print(f'minimizewindow address:{c[\"address\"]}')" | while read -r cmd; do hyprctl dispatch "$cmd" >/dev/null; done
  hyprctl dispatch focuswindow "address:$addr" >/dev/null
  hyprctl dispatch togglefloating "address:$addr" >/dev/null
  sleep 0.4
  hyprctl dispatch resizewindowpixel "exact 1160 760,address:$addr" >/dev/null
  hyprctl dispatch movewindowpixel "exact 40 40,address:$addr" >/dev/null
  sleep "$sleep_s"
  local geo=$(hyprctl -j clients | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if c['address'] == '$addr':
        print(f\"{c['at'][0]},{c['at'][1]} {c['size'][0]}x{c['size'][1]}\"); break")
  grim -g "$geo" "$SHOTDIR/$out" && echo "captured $out ($geo)"
  kill_licoman
}

case "$1" in
  live-dark)
    launch_and_capture "" "p05-live-main-dark.png" 3 "Licoman" ;;
  reads-dark)
    launch_and_capture "reads" "p05-reads-dark.png" 6 "Licoman" ;;
  reads-details)
    launch_and_capture "reads" "p05-source-details.png" 5 "Licoman" ;;
  reads-light)
    launch_and_capture "light" "p05-reads-light.png" 3 "Licoman" ;;
  reads-power)
    launch_and_capture "reads-power" "p05-power-absent-dark.png" 6 "Licoman" ;;
  reads-apps)
    launch_and_capture "reads-apps" "p05-apps-detail-alacritty.png" 6 "Applications" ;;
  *) echo "usage: capture.sh {live-dark|reads-dark|reads-details|reads-light|reads-apps}" ;;
esac

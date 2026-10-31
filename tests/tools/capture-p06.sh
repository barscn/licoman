#!/bin/bash
# Phase 06 screenshot harness (same flow as capture-p05.sh: float -> resize ->
# move -> focus -> grim; kill stale instances first; title-based targeting).
#
# Captures the Phase 06 write-engine demos. **Only run when the desktop is
# free** — never while a fullscreen game or the user's active work is on
# screen (Phase 05 lesson).
#
# Usage: tests/tools/capture-p06.sh [shot ...]
#   writes          staged service+file edits, apply bar, Power category
#   writes-apply    rows in Applying/Applied states (post-apply timing)
#   writes-conflict staged row changed externally (Reload/Compare)
#   writes-permission  permission-held row (lock, Authorize…)
#   writes-light    light theme while the write states are visible
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
  local demo="$1" out="$2" sleep_s="$3" title_sub="$4" height="${5:-760}" move_y="${6:-40}"
  kill_licoman
  ( setsid env LICOMAN_DEMO="$demo" /home/baris/Work/licoman/target/release/licoman >/tmp/lc06-shot.log 2>&1 & )
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

# Tall-content capture on a temporary headless output (Phase 01 D13
# technique): the virtual monitor can be taller than the physical screen,
# so a long settings list fits without scrolling.
capture_tall() {
  local demo="$1" out="$2" sleep_s="${3:-5.5}"
  local VIRT=P06VIRT
  kill_licoman
  hyprctl output create headless "$VIRT" >/dev/null
  sleep 1
  hyprctl keyword monitor "$VIRT,1160x1620@60,2560x0,1" >/dev/null
  sleep 0.5
  ( setsid env LICOMAN_DEMO="$demo" /home/baris/Work/licoman/target/release/licoman >/tmp/lc06-shot.log 2>&1 & )
  sleep 2
  local addr=$(hyprctl -j clients | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if 'licoman' in (c.get('initialClass') or '').lower():
        print(c['address']); break
")
  if [ -z "$addr" ]; then
    echo "NO WINDOW for $demo ($out)"; hyprctl output remove "$VIRT" >/dev/null; kill_licoman; return 1
  fi
  local ws=$(hyprctl -j workspaces | python3 -c "
import json,sys
print(next(w['id'] for w in json.load(sys.stdin) if w['monitor']=='$VIRT'))")
  hyprctl dispatch movetoworkspacesilent "$ws,address:$addr" >/dev/null
  # The tiled window auto-fills the tall output — no float/resize needed.
  sleep "$sleep_s"
  # Output-local capture; crop to the tiled window's geometry (the compositor
  # bar and the tiling gaps stay out of the frame).
  local geo=$(hyprctl -j clients | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if 'licoman' in (c.get('initialClass') or '').lower():
        print(c['at'][0]-2560, c['at'][1], c['size'][0], c['size'][1]); break")
  read -r gx gy gw gh <<<"$geo"
  grim -o "$VIRT" "/tmp/$out" && \
    magick "/tmp/$out" -crop ${gw}x${gh}+${gx}+${gy} +repage "$SHOTDIR/$out" && \
    echo "captured $out on $VIRT (${gw}x${gh} at ${gx},${gy})"
  rm -f "/tmp/$out"
  hyprctl output remove "$VIRT" >/dev/null
  kill_licoman
}

which="${1:-all}"
case "$which" in
  writes)             launch_and_capture "writes" "p06-writes-dark.png" 5.5 "Licoman" ;;
  writes-conflict)    launch_and_capture "writes-conflict" "p06-writes-conflict.png" 5.5 "Licoman" ;;
  writes-permission)  capture_tall "writes-permission" "p06-writes-permission.png" ;;
  writes-light)       launch_and_capture "writes-light" "p06-writes-light.png" 5.5 "Licoman" ;;
  all)
    launch_and_capture "writes" "p06-writes-dark.png" 5.5 "Licoman"
    launch_and_capture "writes-conflict" "p06-writes-conflict.png" 5.5 "Licoman"
    capture_tall "writes-permission" "p06-writes-permission.png"
    launch_and_capture "writes-light" "p06-writes-light.png" 5.5 "Licoman"
    ;;
esac
kill_licoman

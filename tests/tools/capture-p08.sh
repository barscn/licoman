#!/usr/bin/env bash
# Phase 08 capture harness: virtualized settings list + live Xfconf rows.
# Hyprland 0.57: capture by the window's actual geometry (no float/resize —
# the 0.57 dispatch shim + headless-output changes made the old flow fragile;
# same approach as capture-p07.sh).
set -u
cd "$(dirname "$0")/../.." || exit 1
OUT="docs/screenshots"
BIN="./target/release/licoman"
mkdir -p "$OUT"

shot() {
    local name="$1"; shift
    local demo="$1"; shift
    local nav="$1"; shift
    local title_filter="${1:-}"
    [ -n "$title_filter" ] && shift
    local sleep_s="${1:-4}"
    if [ -n "$title_filter" ] && [ "$title_filter" = "--" ]; then title_filter=""; fi
    # title_filter is positional 4; sleep_s positional 5 when 4 args given
    # Pre-clean: kill stale licoman instances by exact binary path.
    pkill -xf "$PWD/$BIN" 2>/dev/null && sleep 0.5
    LICOMAN_DEMO="$demo" LICOMAN_LOG=info LICOMAN_CAPTURE_NAV="$nav" "$BIN" > "/tmp/licoman-p08-$name.log" 2>&1 &
    local pid=$!
    sleep 1.2
    # Wait for the window to appear (up to ~6 s). When several Licoman
    # windows open, pick by title substring (Phase 03 lesson) and minimize
    # the others before capturing.
    local addr=""
    for _ in $(seq 1 24); do
        addr=$(hyprctl -j clients 2>/dev/null | python3 -c "
import json,sys
try:
    clients=json.load(sys.stdin)
except Exception:
    sys.exit(0)
licoman=[c for c in clients if 'licoman' in c.get('class','').lower() and not c.get('hidden')]
tf='$title_filter'
picked=None
for c in licoman:
    if tf and tf.lower() in c.get('title','').lower():
        picked=c; break
# Fall back to the window owned by the process this shot just launched
# (stale instances from earlier runs must never win the pick).
if picked is None and not tf:
    mine=[c for c in licoman if c.get('pid')==$pid]
    if len(mine)==1:
        picked=mine[0]
if picked is not None:
    print(picked['address'])
" )
        [ -n "$addr" ] && break
        sleep 0.25
    done
    if [ -n "$addr" ] && [ -n "$title_filter" ]; then
        # Phase 03/07 lesson: minimize every other Licoman window, focus the
        # target — overlapping windows would otherwise pollute the crop.
        for other in $(hyprctl -j clients 2>/dev/null | python3 -c "
import json,sys
addr=sys.argv[1]
for c in json.load(sys.stdin):
    if 'licoman' in c.get('class','').lower() and c.get('address')!=addr and not c.get('hidden'):
        print(c['address'])
" "$addr"); do
            hyprctl dispatch "minimizewindow address:$other" >/dev/null 2>&1
        done
        hyprctl dispatch "focuswindow address:$addr" >/dev/null 2>&1
        sleep 0.3
    fi
    if [ -z "$addr" ]; then
        echo "[$name] no licoman window found"
        kill -9 "$pid" 2>/dev/null
        return 1
    fi
    sleep "$sleep_s"
    # Client geometry (global layout coords).
    local geom
    geom=$(hyprctl -j clients 2>/dev/null | python3 -c "
import json,sys
addr=sys.argv[1]
for c in json.load(sys.stdin):
    if c.get('address')==addr:
        x,y=c['at']; w,h=c['size']
        print(f'{x},{y} {w}x{h}'); break
" "$addr")
    if [ -z "$geom" ]; then
        echo "[$name] no geometry for $addr"
        kill -9 "$pid" 2>/dev/null
        return 1
    fi
    grim -g "$geom" "$OUT/$name.png" && echo "[$name] captured $geom -> $OUT/$name.png"
    # Kill by the client pid hyprctl reported (stale-instance lesson).
    local cpid
    cpid=$(hyprctl -j clients 2>/dev/null | python3 -c "
import json,sys
addr=sys.argv[1]
for c in json.load(sys.stdin):
    if c.get('address')==addr:
        print(c['pid']); break
" "$addr")
    [ -n "$cpid" ] && kill -9 "$cpid" 2>/dev/null
    kill -9 "$pid" 2>/dev/null
    sleep 0.4
}

case "${1:-all}" in
    xfce-live)  shot p08-live-xfconf "" "Desktop/Xfce" 4 ;;
    scale)      shot p08-scale-list scale "Shared/ScaleCatalog" "" 5 ;;
    scale-apps) shot p08-scale-apps scale "Application" "" 5 ;;
    light-scale) shot p08-light-scale light "Shared/Appearance" "" 5 ;;
    rel-main-dark)  shot p09-release-main-dark "" "Shared/Appearance" "" 6 ;;
    rel-apps-dark)  shot p09-release-apps-dark "" "Application/app.mpv" "Applications" 7 ;;
    rel-main-light) shot p09-release-main-light light "Shared/Appearance" "" 6 ;;
    rel-apps-light) shot p09-release-apps-light light-apps "Application/app.mpv" "Applications" 7 ;;
    empty)          shot p09-no-backend empty "" "" 6 ;;
    mpv-live)   shot p08-live-mpv "" "Application/app.mpv" "Applications" 7 ;;
    all)
        shot p08-live-xfconf "" "Desktop/Xfce" 4
        shot p08-scale-list scale "Shared/ScaleCatalog" "" 5
        ;;
    *) echo "usage: $0 [xfce-live|scale|all]"; exit 1 ;;
esac

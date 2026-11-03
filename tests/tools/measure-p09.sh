#!/usr/bin/env bash
# Phase 09 quality measurements (09 plan §4): idle CPU, idle memory and
# first-frame startup latency of the release build on the reference
# machine. Read-only /proc sampling plus a settle period so the numbers
# describe the steady idle state, not startup allocation. The pointer is
# parked at the screen corner first — hover-driven repaints are not idle.
set -u
cd "$(dirname "$0")/../.." || exit 1
BIN="./target/release/licoman"
OUT="${1:-/tmp/licoman-measure.txt}"
MODE="${2:-live}"          # live | scale | empty
SAMPLE_SECONDS="${3:-30}"

: > "$OUT"
pkill -xf "$PWD/$BIN" 2>/dev/null; sleep 0.5
rm -f /tmp/licoman-measure-run.log

LICOMAN_LOG=info LICOMAN_DEMO="$MODE" "$BIN" >> /tmp/licoman-measure-run.log 2>&1 &
APP_PID=$!
# Wait for the first frame log line (startup instrumentation), else 8 s.
for _ in $(seq 1 32); do
    if grep -q "first frame rendered" /tmp/licoman-measure-run.log 2>/dev/null; then
        break
    fi
    sleep 0.25
done
grep "first frame rendered" /tmp/licoman-measure-run.log | tail -1 >> "$OUT"

# Settle: let the window paint its initial passes and discovery finish;
# park the pointer away from the window (hover is not idle).
sleep 5
hyprctl dispatch movecursor 1900 30 >/dev/null 2>&1 || true
sleep 2

rss() { awk '/VmRSS/{print $2}' "/proc/$APP_PID/status" 2>/dev/null; }
ticks() { awk '{print $14+$15}' "/proc/$APP_PID/stat" 2>/dev/null; }

RSS_SETTLED=$(rss)
START_TICKS=$(ticks)
START_AT=$(date +%s.%N)
sleep "$SAMPLE_SECONDS"
END_TICKS=$(ticks)
END_AT=$(date +%s.%N)
RSS_AFTER=$(rss)
sleep 10
RSS_STABLE=$(rss)

CPU_PCT=$(python3 -c "
start_ticks=$START_TICKS or 0; end_ticks=$END_TICKS or 0
seconds=$END_AT-$START_AT
print(f'{(end_ticks-start_ticks)/100.0/seconds*100:.2f}' if seconds>0 else 'n/a')
")
cat >> "$OUT" <<EOF
mode=$MODE sample_seconds=$SAMPLE_SECONDS settle_before=7s pointer_parked=yes
idle_cpu_mean_percent=$CPU_PCT   (utime+stime over the window; 100%=one core)
rss_settled_kb=${RSS_SETTLED:-n/a}
rss_after_sample_kb=${RSS_AFTER:-n/a}
rss_10s_later_kb=${RSS_STABLE:-n/a}   (stability check: growth here = leak or churn)
build_mode=release binary=$BIN reference=$(uname -sr), $(grep -m1 "model name" /proc/cpuinfo | sed 's/.*: //')
EOF

pkill -xf "$PWD/$BIN" 2>/dev/null
sleep 0.3
cat "$OUT"

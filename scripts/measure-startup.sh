#!/usr/bin/env bash
# Measures startup time (process start -> first paint) and idle memory of the
# release binary. Usage: scripts/measure-startup.sh [--build] [runs]
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "${1:-}" == "--build" ]]; then
  pnpm tauri build --bundles appimage,rpm
  shift
fi
RUNS="${1:-5}"
BIN=src-tauri/target/release/tulkabd
[[ -x "$BIN" ]] || BIN=target/release/tulkabd
[[ -x "$BIN" ]] || { echo "release binary not found; run with --build" >&2; exit 1; }

descendants() { # all descendant PIDs of $1
  local p c
  for c in $(pgrep -P "$1" 2>/dev/null || true); do echo "$c"; descendants "$c"; done
}

times=()
for i in $(seq "$RUNS"); do
  out=$(mktemp)
  TULKA_STARTUP_PROBE=1 "$BIN" >"$out" 2>/dev/null &
  pid=$!
  for _ in $(seq 100); do grep -q TULKA_READY_MS "$out" && break; sleep 0.1; done
  ms=$(sed -n 's/^TULKA_READY_MS=//p' "$out" | head -1)
  if [[ $i -eq 1 ]]; then
    sleep 5
    total=0
    for p in $pid $(descendants "$pid"); do
      kb=$(awk '/^VmRSS/{print $2}' "/proc/$p/status" 2>/dev/null || echo 0)
      total=$((total + kb))
    done
    echo "idle RSS (app + children, 5 s after ready): $((total / 1024)) MB"
  fi
  kill "$pid" 2>/dev/null || true; pkill -P "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  rm -f "$out"
  [[ -n "$ms" ]] || { echo "run $i: no ready signal" >&2; continue; }
  echo "run $i: ${ms} ms"
  times+=("$ms")
done
printf '%s\n' "${times[@]}" | sort -n | awk '{a[NR]=$1} END{print "median: " (NR%2?a[(NR+1)/2]:(a[NR/2]+a[NR/2+1])/2) " ms (n=" NR ")"}'

#!/usr/bin/env bash
# Measures startup time (process start -> first paint of the restored
# workspace) and idle memory of the release binary.
# Usage: scripts/measure-startup.sh [--build] [--memory]
#          [--session FILE | --no-session] [runs]
# --memory launches once, waits 5 s after the ready signal and prints the PSS
# (shared pages counted once) of the app and each descendant, then
# TOTAL_PSS_MB=<n>.
# --session FILE seeds session.db with the session JSON in FILE before every
# run; --no-session removes it before every run. With either, the user's own
# session.db is moved aside first and put back on exit. Without them, runs
# start from whatever session.db the app left.
set -euo pipefail
cd "$(dirname "$0")/.."
BUILD=0
MEMORY=0
SESSION_MODE=keep
SESSION_JSON=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --build) BUILD=1 ;;
    --memory) MEMORY=1 ;;
    --session)
      [[ -n "${2:-}" ]] || { echo "--session needs a JSON file" >&2; exit 1; }
      SESSION_MODE=seed
      SESSION_JSON=$(realpath "$2")
      [[ -r "$SESSION_JSON" ]] || { echo "cannot read $2" >&2; exit 1; }
      shift
      ;;
    --no-session) SESSION_MODE=none ;;
    *) break ;;
  esac
  shift
done
RUNS="${1:-5}"
if [[ $BUILD -eq 1 ]]; then
  pnpm tauri build --bundles appimage,rpm
fi
BIN=src-tauri/target/release/tulkabd
[[ -x "$BIN" ]] || BIN=target/release/tulkabd
[[ -x "$BIN" ]] || { echo "release binary not found; run with --build" >&2; exit 1; }

# Same place as SessionStore::open_default (directories::ProjectDirs on Linux).
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/tulkabd"
SESSION_DB="$DATA_DIR/session.db"
BACKUP=

remove_session() { rm -f "$SESSION_DB" "$SESSION_DB-wal" "$SESSION_DB-shm"; }

restore_user_session() {
  [[ -n "$BACKUP" ]] || return 0
  remove_session
  local f
  for f in "$BACKUP"/*; do
    if [[ -e "$f" ]]; then mv "$f" "$DATA_DIR/"; fi
  done
  rmdir "$BACKUP"
}

if [[ $SESSION_MODE != keep ]]; then
  if [[ $SESSION_MODE == seed ]]; then
    command -v sqlite3 >/dev/null || { echo "--session needs the sqlite3 CLI" >&2; exit 1; }
  fi
  mkdir -p "$DATA_DIR"
  BACKUP=$(mktemp -d "$DATA_DIR/session-backup.XXXXXX")
  for f in "$SESSION_DB" "$SESSION_DB-wal" "$SESSION_DB-shm"; do
    if [[ -e "$f" ]]; then mv "$f" "$BACKUP/"; fi
  done
  trap restore_user_session EXIT
fi

# Puts session.db in the state the mode asks for, before each launch. Same
# layout as crates/tulka-store/src/session.rs (user_version 1).
prepare_session() {
  [[ $SESSION_MODE == keep ]] && return 0
  remove_session
  [[ $SESSION_MODE == seed ]] || return 0
  sqlite3 "$SESSION_DB" "
CREATE TABLE session (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    json TEXT NOT NULL,
    saved_at INTEGER NOT NULL
);
INSERT INTO session (id, json, saved_at)
VALUES (1, CAST(readfile('${SESSION_JSON//\'/\'\'}') AS TEXT), CAST(unixepoch('subsec') * 1000 AS INTEGER));
PRAGMA user_version = 1;
PRAGMA journal_mode = WAL;" >/dev/null
}

descendants() { # all descendant PIDs of $1
  local p c
  for c in $(pgrep -P "$1" 2>/dev/null || true); do echo "$c"; descendants "$c"; done
}

if [[ $MEMORY -eq 1 ]]; then
  prepare_session
  out=$(mktemp)
  TULKA_STARTUP_PROBE=1 "$BIN" >"$out" 2>/dev/null &
  pid=$!
  for _ in $(seq 100); do grep -q TULKA_READY_MS "$out" && break; sleep 0.1; done
  grep -q TULKA_READY_MS "$out" || { echo "no ready signal" >&2; kill "$pid" 2>/dev/null || true; rm -f "$out"; exit 1; }
  sleep 5
  total_kb=0
  for p in $pid $(descendants "$pid"); do
    kb=$(awk '/^Pss:/{print $2}' "/proc/$p/smaps_rollup" 2>/dev/null || true)
    kb=${kb:-0}
    echo "pid $p $(cat "/proc/$p/comm" 2>/dev/null || echo '?'): $((kb / 1024)) MB PSS"
    total_kb=$((total_kb + kb))
  done
  pkill -P "$pid" 2>/dev/null || true; kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  rm -f "$out"
  echo "TOTAL_PSS_MB=$((total_kb / 1024))"
  exit 0
fi

times=()
for i in $(seq "$RUNS"); do
  prepare_session
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

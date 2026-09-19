#!/usr/bin/env bash
# Build, and be loud about failure.
#
# A running instance holds a lock on the exe, so cargo fails to replace it —
# and filtering the output through grep made that look like success. The stale
# binary that produced cost an entire debugging round trip. Always check the
# exit status, and always report the binary's timestamp.
set -o pipefail
taskkill //F //IM discord-taskbar.exe >/dev/null 2>&1
sleep 1

before=$(stat -c '%Y' target/release/discord-taskbar.exe 2>/dev/null || echo 0)
# --all-targets, because `--examples` alone does NOT build the [[bin]]
# target: that is what left a stale exe running for a whole round trip.
cargo build --release --all-targets "$@" 2>&1 | tail -40
status=$?
after=$(stat -c '%Y' target/release/discord-taskbar.exe 2>/dev/null || echo 0)

if [ "$status" -ne 0 ]; then
  echo "BUILD FAILED (exit $status) - binary NOT updated"
  exit "$status"
fi

if [ "$before" = "$after" ]; then
  echo "note: binary unchanged (nothing to relink)"
fi
echo "BUILD OK - binary written $(date -d "@$after" '+%H:%M:%S')"

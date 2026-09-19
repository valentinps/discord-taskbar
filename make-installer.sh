#!/usr/bin/env bash
#
# Build the single-file Windows installer.
#
# Order matters: the installer embeds the application binary with
# include_bytes!, so the application has to exist before the installer is
# compiled. Cargo cannot express that dependency, which is why this is a
# script rather than a build.rs.
#
# Output: dist/DiscordTaskbarSetup.exe

set -o pipefail
cd "$(dirname "$0")" || exit 1

APP=target/release/discord-taskbar.exe
PAYLOAD=installer/payload/discord-taskbar.exe
OUT=dist/DiscordTaskbarSetup.exe

# A running copy holds its own file open, and cargo cannot overwrite it.
taskkill //F //IM discord-taskbar.exe >/dev/null 2>&1
taskkill //F //IM DiscordTaskbarSetup.exe >/dev/null 2>&1
sleep 1

echo "[1/3] building the application"
cargo build --release -p discord-taskbar --bin discord-taskbar 2>&1 | tail -20
if [ ! -f "$APP" ]; then
    echo "BUILD FAILED - $APP missing"
    exit 1
fi

echo "[2/3] embedding $(stat -c '%s' "$APP") bytes as the payload"
mkdir -p installer/payload dist
cp "$APP" "$PAYLOAD"

echo "[3/3] building the installer"
# Touch the source so include_bytes! is re-read; cargo tracks the .rs file,
# not the bytes it pulls in, so a payload-only change would otherwise be
# silently skipped and ship yesterday's application.
touch installer/src/actions.rs
cargo build --release -p discord-taskbar-installer 2>&1 | tail -20
status=$?

if [ "$status" -ne 0 ] || [ ! -f target/release/DiscordTaskbarSetup.exe ]; then
    echo "BUILD FAILED (exit $status)"
    exit 1
fi

cp target/release/DiscordTaskbarSetup.exe "$OUT"
echo
echo "OK  $OUT  ($(stat -c '%s' "$OUT") bytes, built $(date '+%H:%M:%S'))"

#!/usr/bin/env bash
# Tab memory end to end (Copper#131, #134, #181, #127): the `sleep` and
# `sleep-restore` self-test phases in one test world, on a private headless
# weston, with a throwaway HOME (nothing of yours is read or written).
#
#   OUT=/tmp/copper-sleep-e2e   where the PNG and logs go
#   CARGO_TARGET_DIR            as usual (default: target)
#   WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1  where bubblewrap can't run
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../../.."
out=${OUT:-/tmp/copper-sleep-e2e}
target=${CARGO_TARGET_DIR:-target}
cargo build -q -p copper-app || exit 1
bin="$(realpath "$target/debug/copper")"

rm -rf "$out"
mkdir -p "$out"
home="$(mktemp -d)"
rt="$home/rt"
mkdir -p "$rt" && chmod 700 "$rt"
export HOME="$home" XDG_RUNTIME_DIR="$rt" XDG_DATA_HOME="$home/data" XDG_CONFIG_HOME="$home/config" XDG_CACHE_HOME="$home/cache"
# A session bus of its own: WebKit's sandbox waits on one without it.
export DBUS_SESSION_BUS_ADDRESS="unix:path=$rt/bus"
dbus-daemon --session --address="$DBUS_SESSION_BUS_ADDRESS" --fork --print-pid </dev/null >"$rt/dbus.pid"
sock="copper-sleep-$$"
weston --backend=headless --renderer=pixman --socket="$sock" --width=1600 --height=1000 \
  --idle-time=0 </dev/null >"$out/weston.log" 2>&1 &
wpid=$!
trap 'kill $wpid "$(cat "$rt/dbus.pid")" 2>/dev/null; rm -rf "$home"' EXIT
for _ in $(seq 100); do [ -S "$rt/$sock" ] && break; sleep 0.1; done
export WAYLAND_DISPLAY="$sock" GDK_BACKEND=wayland NO_AT_BRIDGE=1 COPPER_WORLD=sleep-e2e
export COPPER_LOG=${COPPER_LOG:-warn,copper_app=info,copper_shell_core=debug,copper::crumb=info}

status=0
timeout 300 "$bin" --selftest-ui sleep --out "$out" 2>"$out/sleep.err" | tee "$out/sleep.log" || status=1
timeout 120 "$bin" --selftest-ui sleep-restore --out "$out" 2>"$out/restore.err" | tee "$out/restore.log" || status=1
grep -hE "webcontent-loop|pin-reload|pages weighed|going to sleep|left asleep" "$out"/*.err | cut -c1-220 | tail -20
exit $status

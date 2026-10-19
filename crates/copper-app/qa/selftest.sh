#!/usr/bin/env bash
# The shell's UI self-test on a private headless weston (run on Linux, e.g.
# `bin/dbx crates/copper-app/qa/selftest.sh`). Builds `copper`, runs the
# fresh → restore phases in one test world and the Mac-session phase in
# another, and leaves the screenshots in $OUT.
#
#   OUT=/tmp/copper-m2-shots   where the PNGs and logs go
#   CARGO_TARGET_DIR           as usual (default: target)
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../../.."
out=${OUT:-/tmp/copper-m2-shots}
target=${CARGO_TARGET_DIR:-target}
cargo build -q -p copper-app || exit 1
bin="$target/debug/copper"

rm -rf "$out"
mkdir -p "$out"
rt="/tmp/copper-m2-rt-$(id -u)"
mkdir -p "$rt" && chmod 700 "$rt"
export XDG_RUNTIME_DIR="$rt"
sock="copper-m2-$$"
weston --backend=headless --renderer=pixman --socket="$sock" --width=1600 --height=1000 \
  --idle-time=0 </dev/null >"$out/weston.log" 2>&1 &
wpid=$!
trap 'kill $wpid 2>/dev/null' EXIT
for _ in $(seq 100); do [ -S "$rt/$sock" ] && break; sleep 0.1; done
export WAYLAND_DISPLAY="$sock" GDK_BACKEND=wayland NO_AT_BRIDGE=1
export COPPER_LOG=${COPPER_LOG:-warn,copper_app=info,copper_shell_core=info}

world() {
  export COPPER_WORLD=$1
  rm -rf "$HOME/.local/share/copper-$1" "$HOME/.config/copper-$1" "$HOME/.cache/copper-$1"
}

status=0
world m2test
timeout 420 "$bin" --selftest-ui fresh --out "$out" 2>"$out/fresh.err" | tee "$out/fresh.log" || status=1
timeout 180 "$bin" --selftest-ui restore --out "$out" 2>"$out/restore.err" | tee "$out/restore.log" || status=1
world m2mac
mkdir -p "$HOME/.local/share/copper-m2mac"
cp fixtures/mac/session.json "$HOME/.local/share/copper-m2mac/session.json"
timeout 180 "$bin" --selftest-ui mac --out "$out" 2>"$out/mac.err" | tee "$out/mac.log" || status=1
# The --screenshot hook on its own (COPPER_SCREENSHOT works the same way).
world m2shot
timeout 60 "$bin" --screenshot "$out/40-screenshot-hook.png" \
  'data:text/html,<title>Screenshot hook</title><body style="font:20px sans-serif;margin:30vh auto;width:30em">Saved by --screenshot.</body>' \
  >"$out/shot.log" 2>"$out/shot.err" || status=1
cat "$out/shot.log"
ls "$out"/*.png
exit $status

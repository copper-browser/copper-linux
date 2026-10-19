#!/usr/bin/env bash
# The addons lane's self-test on a private headless weston (Linux): builds
# `copper`, runs `COPPER_SELFTEST=addons` in a throwaway test world with the
# hello fixture (qa/addons/hello), and leaves the PNGs and logs in $OUT.
#
#   OUT=/tmp/copper-addons-selftest   where the PNGs and logs go
#   CARGO_TARGET_DIR                  as usual (default: target)
#   DEV=1                             also run the fixture as a developer
#                                     folder (COPPER_ADDONS_DEV_DIR)
#
# Prints the PASS/FAIL lines; exits non-zero on any failure.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../../.."
out=${OUT:-/tmp/copper-addons-selftest}
target=${CARGO_TARGET_DIR:-target}
timeout 1800 cargo build -q -p copper-app || exit 1
bin="$target/debug/copper"
fixture="$PWD/crates/copper-app/qa/addons/hello"

rm -rf "$out"
mkdir -p "$out"
rt="/tmp/copper-addons-rt-$(id -u)-$$"
mkdir -p "$rt" && chmod 700 "$rt"
export XDG_RUNTIME_DIR="$rt"
sock="copper-addons-$$"
timeout 900 weston --backend=headless --renderer=pixman --socket="$sock" --width=1440 --height=900 \
  --idle-time=0 </dev/null >"$out/weston.log" 2>&1 &
wpid=$!
trap 'kill $wpid 2>/dev/null; rm -rf "$rt"' EXIT
for _ in $(seq 100); do [ -S "$rt/$sock" ] && break; sleep 0.1; done
export WAYLAND_DISPLAY="$sock" GDK_BACKEND=wayland NO_AT_BRIDGE=1 GTK_A11Y=none
export COPPER_LOG=${COPPER_LOG:-warn,copper_app=info}
export COPPER_SELFTEST=addons COPPER_ADDONS_FIXTURE="$fixture"

world() {
  export COPPER_WORLD=$1
  rm -rf "$HOME/.local/share/copper-$1" "$HOME/.config/copper-$1" "$HOME/.cache/copper-$1"
}

status=0
world addons-qa
COPPER_SELFTEST_OUT="$out" timeout 300 "$bin" 2>"$out/addons.err" | tee "$out/addons.log" || status=1
if [ "${DEV:-0}" = 1 ]; then
  world addons-qa
  mkdir -p "$out/dev"
  COPPER_ADDONS_DEV_DIR="$fixture" COPPER_SELFTEST_OUT="$out/dev" timeout 300 "$bin" \
    2>"$out/dev/addons.err" | tee "$out/dev/addons.log" || status=1
fi
grep -q '^FAIL' "$out"/addons.log "$out"/dev/addons.log 2>/dev/null && status=1
ls "$out"/*.png
exit $status

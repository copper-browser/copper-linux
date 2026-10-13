#!/usr/bin/env bash
# creds-ui end to end on the devbox (headless weston from bin/dbx):
#   a private session D-Bus + gnome-keyring-daemon (empty password),
#   the login fixtures served over HTTP on localhost,
#   the Bitwarden mock CLI, a Chrome user-data root under XDG_CONFIG_HOME,
#   then `copper` with COPPER_SELFTEST=forms, then secret-tool checks.
#
#   crates/copper-app/src/forms/e2e/run.sh <login.html> [out dir]
#
# COPPER_BIN: the binary (default target/debug/copper; it must be where the
# AppArmor userns profile allows bubblewrap).
#
# Never touches the user's keyring, config or data: HOME, XDG_* and the bus
# are all private to the run.
set -euo pipefail
REPO=$(cd "$(dirname "$0")/../../../../.." && pwd)
LOGIN_HTML=${1:?login.html (copper-ref docs/fixtures/login.html)}
OUT=${2:-/tmp/copper-forms-e2e/out}
W=/tmp/copper-forms-e2e
E2E="$REPO/crates/copper-app/src/forms/e2e"

cleanup() {
  [ -n "${HTTP_PID:-}" ] && kill "$HTTP_PID" 2>/dev/null || true
  [ -n "${KEYRING_PID:-}" ] && kill "$KEYRING_PID" 2>/dev/null || true
  [ -n "${BUS_PID:-}" ] && kill "$BUS_PID" 2>/dev/null || true
}
trap cleanup EXIT

rm -rf "$W"
mkdir -p "$W"/{site,home,run,config,data,mock} "$OUT"
chmod 700 "$W/run"
rm -f "$OUT"/*.png "$OUT"/result-*.json

# — pages —
cp "$LOGIN_HTML" "$W/site/login.html"
cp "$E2E/login-fail.html" "$E2E/checkout.html" "$W/site/"
PORT=$(python3 -c "import socket; s=socket.socket(); s.bind(('127.0.0.1', 0)); print(s.getsockname()[1])")
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$W/site" >"$W/http.log" 2>&1 &
HTTP_PID=$!
for _ in $(seq 1 50); do curl -sf "http://127.0.0.1:$PORT/login.html" >/dev/null && break; sleep 0.1; done

# — a private Secret Service —
BUS_ADDR=$(dbus-daemon --session --fork --print-address=1 --print-pid=3 \
  --address="unix:path=$W/run/bus" 3>"$W/run/bus.pid")
BUS_PID=$(cat "$W/run/bus.pid")
export DBUS_SESSION_BUS_ADDRESS="$BUS_ADDR"
export HOME="$W/home" XDG_DATA_HOME="$W/data" XDG_CONFIG_HOME="$W/config"
printf '\n' | env -i PATH="$PATH" HOME="$HOME" XDG_RUNTIME_DIR="$W/run" \
  XDG_DATA_HOME="$HOME/.local/share" DBUS_SESSION_BUS_ADDRESS="$BUS_ADDR" \
  gnome-keyring-daemon --foreground --unlock --components=secrets >"$W/keyring.log" 2>&1 &
KEYRING_PID=$!
for _ in $(seq 1 100); do
  if dbus-send --session --print-reply --dest=org.freedesktop.DBus /org/freedesktop/DBus \
      org.freedesktop.DBus.NameHasOwner string:org.freedesktop.secrets 2>/dev/null | grep -q 'boolean true'; then
    break
  fi
  sleep 0.1
done

# — Chrome, with its v11 key in the keyring as Chrome keeps it —
V11=$(python3 "$E2E/chrome_root.py" "$REPO/crates/copper-import/tests/fixtures/flow-chrome" \
  "$XDG_CONFIG_HOME/google-chrome")
printf '%s' "$V11" | secret-tool store --label="Chrome Safe Storage" application chrome
unset V11

# — Bitwarden: the mock CLI —
export COPPER_BW_PATH="$REPO/crates/copper-creds/tests/fixtures/bw-mock/bw"
export BW_MOCK_STATE="$W/mock/state.json"
python3 "$E2E/bw_state.py" "$COPPER_BW_PATH" "$BW_MOCK_STATE"
# No 1Password CLI on this box: the page shows how to get one.
export COPPER_OP_PATH=/nonexistent

# — Copper —
export COPPER_WORLD=forms-e2e COPPER_SELFTEST=forms
export COPPER_FORMS_BASE="http://localhost:$PORT/" COPPER_FORMS_OUT="$OUT"
set +e
timeout 300 "${COPPER_BIN:-$REPO/target/debug/copper}" 2>"$W/copper.log"
CODE=$?
set -e
grep -E '^(PASS|FAIL)|selftest|screenshot|import summary' "$W/copper.log" || true

# — what the keyring holds now (attributes only; secrets are not printed) —
echo "--- secret-tool search app=Copper"
secret-tool search --all app Copper 2>&1 | grep -E '^(\[|label|attribute)' || true
echo "--- secret-tool lookup (length only)"
for who in "localhost ann@example.com" "example.com ann" "accounts.example.org bob"; do
  set -- $who
  v=$(secret-tool lookup app Copper origin "$1" username "$2" 2>/dev/null || true)
  echo "$1 $2 -> ${#v} chars"
done
echo "copper exit: $CODE"

# — second run: a session bus with no Secret Service and nothing to activate —
kill "$KEYRING_PID" 2>/dev/null || true
kill "$BUS_PID" 2>/dev/null || true
KEYRING_PID= BUS_PID=
cat >"$W/run/bare-bus.conf" <<CONF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=$W/run/bare-bus</listen>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
CONF
BUS_ADDR=$(dbus-daemon --config-file="$W/run/bare-bus.conf" --fork --print-address=1 --print-pid=3 3>"$W/run/bare.pid")
BUS_PID=$(cat "$W/run/bare.pid")
export DBUS_SESSION_BUS_ADDRESS="$BUS_ADDR"
export COPPER_WORLD=forms-e2e-nokeyring COPPER_FORMS_PHASE=nokeyring
set +e
timeout 120 "${COPPER_BIN:-$REPO/target/debug/copper}" 2>"$W/copper-nokeyring.log"
CODE2=$?
set -e
grep -E '^(PASS|FAIL)|selftest|screenshot' "$W/copper-nokeyring.log" || true
echo "copper (no keyring) exit: $CODE2"
[ "$CODE" -eq 0 ] && [ "$CODE2" -eq 0 ]

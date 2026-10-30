#!/usr/bin/env bash
# The addons lane against a real Copper Cloud, on a private headless weston
# (Linux). Builds `copper`, makes a throwaway test world that is linked and
# signed in (POST /v1/auth/login with a new device id, then `cloud.json`
# written 0600), and runs `COPPER_SELFTEST=addons-cloud`: the app's own sync
# must bring EXPECT_ID at EXPECT_VERSION, and Settings › About › Install
# must put it in the Agent sidebar. PNGs and logs go to $OUT.
#
#   CLOUD_URL=http://127.0.0.1:8080   the instance (http only for loopback) …
#   CLOUD_KEY=…                       … and its instance key, or instead:
#   LINK_CODE=copper-cloud://HOST:PORT/#k=…&fp=…   (`copper-cloud link-code`)
#   CLOUD_EMAIL=tester@example.com    the account to sign in as
#   CLOUD_PASSWORD_FILE=path          its password (or CLOUD_PASSWORD)
#   EXPECT_ID, EXPECT_VERSION         the addon the cloud hands that account
#   PANE_SCRIPT=path                  optional: drives the addon's UI (see
#                                     src/addons/selftest_cloud.rs)
#   UPDATE_TO=version                 optional: then a newer version arrives …
#   UPDATE_CMD='…'                    … published by this command, run (with
#                                     bash -c) once the lane asks for it
#   UPDATE_PANE_SCRIPT=path           optional: runs after the update
#   OUT=/tmp/copper-addons-cloud-e2e  where the PNGs and logs go
#   WORLD=addons-cloud-qa             the test world (wiped first and after)
#   CARGO_TARGET_DIR                  as usual (default: target)
#
# Never prints the password or the session token. Prints the PASS/FAIL
# lines; exits non-zero on any failure.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../../.." || exit 2
out=${OUT:-/tmp/copper-addons-cloud-e2e}
world=${WORLD:-addons-cloud-qa}
target=${CARGO_TARGET_DIR:-target}
die() { echo "addons-cloud-e2e: $*" >&2; exit 2; }
for t in curl jq python3 weston; do command -v $t >/dev/null || die "needs $t"; done

url=${CLOUD_URL:-}
key=${CLOUD_KEY:-}
fp=""
if [ -n "${LINK_CODE:-}" ]; then
  # copper-cloud://HOST:PORT/#k=KEY&fp=HEX[&tls=off] → url, key, fingerprint.
  read -r url key fp < <(LINK_CODE="$LINK_CODE" python3 - <<'EOF'
import os, urllib.parse
code = "".join(os.environ["LINK_CODE"].split())
head, _, frag = code.partition("#")
f = {k.lower(): urllib.parse.unquote(v) for k, _, v in (p.partition("=") for p in frag.split("&") if p)}
plain = f.get("tls", "").lower() in ("off", "none", "plain", "http")
if head.lower().startswith("copper-cloud://"):
    head = ("http://" if plain else "https://") + head[15:]
print(head.rstrip("/"), f.get("k", ""), f.get("fp", "-"))
EOF
)
  [ "$fp" = "-" ] && fp=""
fi
url=${url%/}
[ -n "$url" ] && [ -n "$key" ] || die "set CLOUD_URL and CLOUD_KEY, or LINK_CODE"
[ -n "${CLOUD_EMAIL:-}" ] || die "set CLOUD_EMAIL"
if [ -n "${CLOUD_PASSWORD_FILE:-}" ]; then
  password=$(cat "$CLOUD_PASSWORD_FILE") || die "can't read CLOUD_PASSWORD_FILE"
else
  password=${CLOUD_PASSWORD:-}
fi
[ -n "$password" ] || die "set CLOUD_PASSWORD_FILE or CLOUD_PASSWORD"
[ -n "${EXPECT_ID:-}" ] && [ -n "${EXPECT_VERSION:-}" ] || die "set EXPECT_ID and EXPECT_VERSION"
if [ -n "${UPDATE_TO:-}" ] && [ -z "${UPDATE_CMD:-}" ]; then die "UPDATE_TO needs UPDATE_CMD"; fi
case "$url" in
  https://*) ;;
  http://127.0.0.1:* | http://localhost:*) ;;
  *) die "CLOUD_URL must be https://…, or http:// on 127.0.0.1 or localhost" ;;
esac
curl_tls=()
if [ -n "$fp" ]; then
  # A pinned self-signed instance: curl can't pin a leaf by hash; the app
  # checks the fingerprint itself.
  curl_tls=(-k)
fi

timeout 1800 cargo build -q -p copper-app || exit 1
bin="$target/debug/copper"

rm -rf "$out"
mkdir -p "$out"
data="$HOME/.local/share/copper-$world"
wipe() { rm -rf "$data" "$HOME/.config/copper-$world" "$HOME/.cache/copper-$world"; }
wipe

# Sign in as a new device; keep only what cloud.json needs.
device=$(python3 -c 'import uuid; print(str(uuid.uuid4()).upper())')
device_lc=$(printf %s "$device" | tr '[:upper:]' '[:lower:]')
body=$(jq -n --arg e "$CLOUD_EMAIL" --arg p "$password" --arg d "$device_lc" \
  '{email:$e, password:$p, device:{id:$d, name:"Copper addons E2E"}}')
answer=$(timeout 30 curl -sS "${curl_tls[@]}" -X POST "$url/v1/auth/login" \
  -H "X-Copper-Instance: $key" -H 'Content-Type: application/json' --data-binary @- <<<"$body")
unset body password
token=$(jq -r '.token // empty' <<<"$answer")
[ -n "$token" ] || die "sign-in refused: $(jq -c 'del(.token)' <<<"$answer" 2>/dev/null | head -c 300)"
mkdir -p "$data"
chmod 700 "$data"
umask 077
jq -n --arg dev "$device" --arg url "$url" --arg key "$key" --arg fp "$fp" --arg tok "$token" \
  --argjson a "$answer" '{
    deviceId: $dev,
    deviceName: "Copper addons E2E",
    link: ({url: $url, key: $key} + (if $fp == "" then {} else {fingerprint: $fp} end)),
    account: {
      userId: ($a.user.id | ascii_upcase),
      email: $a.user.email,
      displayName: ($a.user.display_name // $a.user.email),
      deviceId: (($a.device.id // $dev) | ascii_upcase)
    },
    token: $tok,
    sync: {on: false}
  }' >"$data/cloud.json"
chmod 600 "$data/cloud.json"
umask 022
unset answer
echo "signed in as $CLOUD_EMAIL on $url (device $device)"

rt="/tmp/copper-addons-cloud-rt-$(id -u)-$$"
mkdir -p "$rt" && chmod 700 "$rt"
export XDG_RUNTIME_DIR="$rt"
sock="copper-addons-cloud-$$"
timeout 1800 weston --backend=headless --renderer=pixman --socket="$sock" --width=1440 --height=900 \
  --idle-time=0 </dev/null >"$out/weston.log" 2>&1 &
wpid=$!
cpid=""
cleanup() {
  [ -n "$cpid" ] && kill "$cpid" 2>/dev/null
  kill "$wpid" 2>/dev/null
  # End the session on the server; the throwaway world goes with it.
  if [ -n "${token:-}" ]; then
    timeout 15 curl -sS "${curl_tls[@]}" -o /dev/null -X POST "$url/v1/auth/logout" \
      -H "X-Copper-Instance: $key" -H "Authorization: Bearer $token" || true
  fi
  [ "${KEEP_WORLD:-0}" = 1 ] || wipe
  rm -rf "$rt"
}
trap cleanup EXIT
for _ in $(seq 100); do [ -S "$rt/$sock" ] && break; sleep 0.1; done
[ -S "$rt/$sock" ] || die "weston didn't start (see $out/weston.log)"

export WAYLAND_DISPLAY="$sock" GDK_BACKEND=wayland NO_AT_BRIDGE=1 GTK_A11Y=none
export COPPER_LOG=${COPPER_LOG:-warn,copper_app=info,copper_cloud=info}
export COPPER_WORLD="$world" COPPER_SELFTEST=addons-cloud COPPER_SELFTEST_OUT="$out"
export COPPER_E2E_EXPECT_ID="$EXPECT_ID" COPPER_E2E_EXPECT_VERSION="$EXPECT_VERSION"
export COPPER_E2E_EXPECT_EMAIL="$CLOUD_EMAIL"
if [ -n "${PANE_SCRIPT:-}" ]; then
  COPPER_E2E_PANE_SCRIPT=$(realpath "$PANE_SCRIPT") || die "no $PANE_SCRIPT"
  export COPPER_E2E_PANE_SCRIPT
fi
if [ -n "${UPDATE_PANE_SCRIPT:-}" ]; then
  COPPER_E2E_UPDATE_PANE_SCRIPT=$(realpath "$UPDATE_PANE_SCRIPT") || die "no $UPDATE_PANE_SCRIPT"
  export COPPER_E2E_UPDATE_PANE_SCRIPT
fi
if [ -n "${UPDATE_TO:-}" ]; then
  export COPPER_E2E_UPDATE_TO="$UPDATE_TO" COPPER_E2E_UPDATE_READY="$out/update-ready"
fi

timeout 900 "$bin" >"$out/cloud-e2e.log" 2>"$out/cloud-e2e.err" &
cpid=$!
tail --pid="$cpid" -n +1 -f "$out/cloud-e2e.log" &
published=0
update_failed=0
while kill -0 "$cpid" 2>/dev/null; do
  if [ -n "${UPDATE_TO:-}" ] && [ $published = 0 ] && [ -f "$out/update-wanted" ]; then
    published=1
    echo "publishing $EXPECT_ID $UPDATE_TO: $UPDATE_CMD"
    if timeout 300 bash -c "$UPDATE_CMD" >"$out/update-cmd.log" 2>&1; then
      touch "$out/update-ready"
    else
      echo "FAIL UPDATE_CMD exited non-zero (see $out/update-cmd.log)"
      update_failed=1
    fi
  fi
  sleep 0.5
done
wait "$cpid"
status=$?
cpid=""
sleep 0.3
# The log must never carry the session token.
if grep -qF "$token" "$out"/*.log "$out"/*.err 2>/dev/null; then
  echo "FAIL the session token appears in a log"
  status=1
fi
grep -q '^FAIL' "$out/cloud-e2e.log" && status=1
[ $update_failed = 1 ] && status=1
ls "$out"/*.png 2>/dev/null
exit $status

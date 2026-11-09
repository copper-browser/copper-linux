#!/usr/bin/env bash
# check.sh — the Roadrunner checks (.woodpecker/ci.yaml, .woodpecker/canvas.yaml). Each mode
# runs INSIDE a ci-dagger container (Debian trixie rust image or Ubuntu 24.04), never on the
# Roadrunner host:
#
#   bash ci/check.sh test     # rustfmt, clippy -D warnings, headless weston + session bus,
#                             # copper-cloud live server on native PostgreSQL, cargo test
#   bash ci/check.sh lint     # shellcheck, desktop entry, AppStream, AppArmor, Flatpak manifest
#   bash ci/check.sh canvas   # the vendored canvas page is the pinned canvas-app build
#
# test expects COPPER_CLOUD_TEST_DATABASE_URL (ci-dagger --pg-admin-url --database-env: an
# isolated database on native PostgreSQL, never a container).
set -euo pipefail
cd "$(dirname "$0")/.."

# copper-cloud server for crates/copper-cloud/tests/live_server.rs: the pinned public release
# (copper-browser/copper-cloud v0.8.1), checked against its published SHA256SUMS.
COPPER_CLOUD_VERSION=0.8.1
COPPER_CLOUD_SHA256_X86_64=344cc217a90758153963c877c65cae1db2aa39ada646fed813f61a45aada81f5
COPPER_CLOUD_SHA256_AARCH64=e6877aa2d49e014d9f8997b8ee45f007271e11d7a70fb41a3febd58876d1d56a

test_suite() {
  cargo fmt --all --check
  cargo clippy --workspace --all-targets --locked -- -D warnings

  # Headless weston (pixman, 1440x900) and a private session bus, as bin/dbx gives the devbox.
  rt=$(mktemp -d /tmp/xdg.XXXXXX)
  chmod 700 "$rt"
  export XDG_RUNTIME_DIR="$rt"
  dbus-daemon --session --address="unix:path=$rt/bus" --fork --print-pid >"$rt/dbus.pid"
  setsid nohup weston --backend=headless --renderer=pixman --socket=copper-wl \
    --width=1440 --height=900 --idle-time=0 </dev/null >"$rt/weston.log" 2>&1 &
  for _ in $(seq 1 100); do [ -S "$rt/copper-wl" ] && break; sleep 0.1; done
  [ -S "$rt/copper-wl" ] || { cat "$rt/weston.log"; exit 1; }
  export WAYLAND_DISPLAY=copper-wl GDK_BACKEND=wayland NO_AT_BRIDGE=1
  export DBUS_SESSION_BUS_ADDRESS="unix:path=$rt/bus"

  # The live copper-cloud test runs against a real server; REQUIRE_LIVE=1 makes a missing
  # server fail instead of skipping.
  case "$(uname -m)" in
    x86_64) arch=x86_64 sum=$COPPER_CLOUD_SHA256_X86_64 ;;
    aarch64) arch=aarch64 sum=$COPPER_CLOUD_SHA256_AARCH64 ;;
    *) echo "no copper-cloud build for $(uname -m)" >&2; exit 1 ;;
  esac
  : "${COPPER_CLOUD_TEST_DATABASE_URL:?run with ci-dagger --pg-admin-url --database-env=COPPER_CLOUD_TEST_DATABASE_URL}"
  tarball="copper-cloud-${COPPER_CLOUD_VERSION}-linux-${arch}.tar.gz"
  dir=$(mktemp -d /tmp/copper-cloud.XXXXXX)
  curl -fsSL --retry 5 --retry-delay 3 -o "$dir/$tarball" \
    "https://github.com/copper-browser/copper-cloud/releases/download/v${COPPER_CLOUD_VERSION}/$tarball"
  echo "$sum  $dir/$tarball" | sha256sum -c -
  tar -xzf "$dir/$tarball" -C "$dir" copper-cloud
  "$dir/copper-cloud" --version
  export COPPER_CLOUD_BIN="$dir/copper-cloud" COPPER_CLOUD_REQUIRE_LIVE=1

  # --no-fail-fast: one failing test binary must not hide the others.
  cargo test --workspace --locked --no-fail-fast || {
    echo "--- weston log" >&2
    cat "$rt/weston.log" >&2 || true
    exit 1
  }
}

lint() {
  shellcheck -s sh install.sh packaging/debian/maintainer-scripts/*
  shellcheck packaging/*.sh packaging/*/*.sh
  desktop-file-validate packaging/share/com.copperbrowser.Copper.desktop
  appstreamcli validate --no-net packaging/share/com.copperbrowser.Copper.metainfo.xml
  apparmor_parser -Q -K -T packaging/debian/copper.apparmor
  # Flatpak manifest parses (with cargo sources from Cargo.lock).
  packaging/flatpak/vendor.sh
  err=$(mktemp)
  flatpak-builder --show-manifest packaging/flatpak/com.copperbrowser.Copper.yml >/dev/null 2>"$err"
  if [ -s "$err" ]; then cat "$err"; exit 1; fi
}

canvas() {
  packaging/sync-canvas-bundle.sh --check --canvas-app \
    "$(cat crates/copper-shell-core/src/canvas/resources/CANVAS_APP_REF)"
}

case "${1:-}" in
  test) test_suite ;;
  lint) lint ;;
  canvas) canvas ;;
  *) echo "usage: ci/check.sh test|lint|canvas" >&2; exit 2 ;;
esac

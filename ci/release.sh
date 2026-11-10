#!/usr/bin/env bash
# release.sh — the Roadrunner release (.woodpecker/release.yaml); same assets as the old
# GitHub release.yml. On the Roadrunner host (bash, git, curl, jq, the dagger CLI; no Rust,
# apt or python there):
#
#   bash ci/release.sh build <part> [vX.Y.Z]   # part: deb-amd64 deb-arm64 flatpak-x86_64 flatpak-aarch64
#                                              # → dist/parts/<part>/ (built in an Ubuntu 24.04 container)
#   bash ci/release.sh assemble [vX.Y.Z]       # → dist/publish/: install.sh, copper-version.json, SHA256SUMS
#   GH_TOKEN=… bash ci/release.sh publish vX.Y.Z   # GitHub release + verify the published bytes
#
# No tag (a manual run) = a dry run for v<Cargo.toml version>: everything is built, smoke-tested
# and assembled; publish only ever runs for a pushed tag. A tag must equal
# [workspace.package] version in Cargo.toml.
#
# Containers (dagger core CLI against the host engine) are Ubuntu 24.04, like the GitHub runners,
# so the .deb's dependencies (dpkg-shlibdeps) match Ubuntu 24.04+ / Debian 13+. arm64 builds run
# in an arm64 container under the engine's qemu (slow, but the same script as amd64; the smoke
# test runs the arm64 binary too). Flatpak builds need bubblewrap, so their exec gets
# --insecure-root-capabilities. The _deb/_flatpak/_assemble modes run inside those containers.
set -euo pipefail

REPO=copper-browser/copper-linux
UBUNTU=public.ecr.aws/docker/library/ubuntu:24.04@sha256:fe81560990fae86dc4743202b2cb4d3df98d0bcf949173cecef15ce1127ef852
PYTHON=public.ecr.aws/docker/library/python:3.12-slim-bookworm@sha256:935c3c5b12357a7e10caa9c1e6ca5692344d97cf229832ef584be2cec9fc03df
RUSTUP_VERSION=1.29.3
RUSTUP_SHA256_X86_64=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55
RUSTUP_SHA256_AARCH64=9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282
CARGO_DEB_VERSION=3.8.0
GH_VERSION=2.103.3
GH_SHA256_AMD64=4c806978ffce1cec70853302a56d70ac6da4f19bae7adca4e1a8ab00633dd91d
GH_SHA256_ARM64=ee32df2323c8847f5fac6cb6b4b69ca75828e9329a8f22f677021f7b3306ce63
APT_BUILD_DEPS="build-essential pkg-config libwebkitgtk-6.0-dev libgtk-4-dev libadwaita-1-dev libsecret-1-dev"

usage='usage: release.sh build <deb-amd64|deb-arm64|flatpak-x86_64|flatpak-aarch64> [vX.Y.Z] | assemble [vX.Y.Z] | publish vX.Y.Z'
cd "$(dirname "$0")/.."

die() { echo "release: $*" >&2; exit 1; }

# meta [tag]: sets version and tag; a tag must match Cargo.toml.
meta() {
  local cargo_version meta_release
  cargo_version=$(awk '/^\[/ { s = ($0 == "[workspace.package]") }
    s && /^version *=/ { gsub(/^version *= *"|".*$/, ""); print; exit }' Cargo.toml)
  [ -n "$cargo_version" ] || die "no [workspace.package] version in Cargo.toml"
  tag=${1:-v$cargo_version}
  version=${tag#v}
  [ "$version" = "$cargo_version" ] ||
    die "tag $tag does not match Cargo.toml [workspace.package] version $cargo_version — bump Cargo.toml (and Cargo.lock) first"
  meta_release=$(sed -n 's/.*<release version="\([^"]*\)".*/\1/p' packaging/share/com.copperbrowser.Copper.metainfo.xml | head -1)
  [ "$meta_release" = "$version" ] ||
    echo "release: warning: com.copperbrowser.Copper.metainfo.xml's newest <release> is $meta_release, not $version" >&2
  echo "release: $tag (version $version)"
}

dagger_core() {
  export DO_NOT_TRACK=1 DAGGER_NO_NAG=1 DAGGER_NO_UPDATE_CHECK=1
  export _EXPERIMENTAL_DAGGER_RUNNER_HOST=${_EXPERIMENTAL_DAGGER_RUNNER_HOST:-unix:///run/dagger-engine/engine.sock}
  dagger --progress=plain core "$@"
}

# ---------------------------------------------------------------- host

build() {
  local part=${1:?$usage} platform kind arch
  meta "${2:-}"
  case "$part" in
    deb-amd64) kind=deb arch=amd64 platform=linux/amd64 ;;
    deb-arm64) kind=deb arch=arm64 platform=linux/arm64 ;;
    flatpak-x86_64) kind=flatpak arch=x86_64 platform=linux/amd64 ;;
    flatpak-aarch64) kind=flatpak arch=aarch64 platform=linux/amd64 ;; # cross: bwrap fails under qemu
    *) die "$usage" ;;
  esac
  rm -rf "dist/parts/$part"
  mkdir -p dist/parts
  local caches=(with-mounted-cache --path=/root/.cargo/registry --cache="copper-linux-cargo-registry-$arch")
  local exec_opts=()
  if [ "$kind" = flatpak ]; then
    # The GNOME SDK + rust-stable extension (~3 GB), kept between releases.
    caches+=(with-mounted-cache --path=/root/.local/share/flatpak --cache="copper-linux-flatpak-$arch")
    exec_opts=(--insecure-root-capabilities)
  fi
  dagger_core container --platform="$platform" \
    from --address="$UBUNTU" \
    with-env-variable --name=HOME --value=/root \
    "${caches[@]}" \
    with-directory --path=/src --source=. --exclude=target,dist \
    with-workdir --path=/src \
    with-exec --args="bash,ci/release.sh,_$kind,$version,$arch" "${exec_opts[@]}" \
    directory --path=/src/dist \
    export --path="./dist/parts/$part"
  ls -la "dist/parts/$part"
}

assemble() {
  meta "${1:-}"
  local commit published_at
  commit=$(git rev-parse HEAD)
  published_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  rm -rf dist/publish
  dagger_core container from --address="$PYTHON" \
    with-directory --path=/src --source=. --exclude=.git,target,dist/publish,dist/tools \
    with-workdir --path=/src \
    with-exec --args="bash,ci/release.sh,_assemble,$version,$tag,$commit,$published_at" \
    directory --path=/src/dist/publish \
    export --path=./dist/publish
  ls -la dist/publish
}

publish() {
  local tag=${1:?$usage}
  : "${GH_TOKEN:?GH_TOKEN must be set}"
  meta "$tag"
  local tools=$PWD/dist/tools gh_arch gh_sha
  case "$(uname -m)" in
    x86_64) gh_arch=amd64 gh_sha=$GH_SHA256_AMD64 ;;
    aarch64) gh_arch=arm64 gh_sha=$GH_SHA256_ARM64 ;;
    *) die "unsupported machine $(uname -m)" ;;
  esac
  local ghdir=gh_${GH_VERSION}_linux_$gh_arch
  mkdir -p "$tools"
  [ -f "$tools/$ghdir.tar.gz" ] || curl -fsSL --retry 3 -o "$tools/$ghdir.tar.gz" \
    "https://github.com/cli/cli/releases/download/v$GH_VERSION/$ghdir.tar.gz"
  echo "$gh_sha  $tools/$ghdir.tar.gz" | sha256sum -c --quiet -
  tar -xzf "$tools/$ghdir.tar.gz" -C "$tools"
  gh() { GH_CONFIG_DIR="$tools/gh-config" GH_PROMPT_DISABLED=1 GH_NO_UPDATE_NOTIFIER=1 "$tools/$ghdir/bin/gh" "$@"; }

  local dist=dist/publish commit gist notes flags=(--latest)
  [ -f "$dist/copper-version.json" ] || die "$dist/copper-version.json is missing; run assemble first"
  commit=$(jq -r .commit "$dist/copper-version.json")
  gist=$(jq -r .notes "$dist/copper-version.json")
  notes=$(mktemp)
  {
    echo "Copper ${version} for Linux — built from ${REPO}@${commit:0:12}."
    echo
    if [ -n "$gist" ]; then echo "$gist"; echo; fi
    echo "**Install**"
    echo
    echo '```sh'
    echo "curl -fsSL https://github.com/${REPO}/releases/latest/download/install.sh | sh"
    echo '```'
    echo
    echo "or download a file below: the \`.deb\` for Ubuntu 24.04+ / Debian 13+ (\`sudo apt install ./copper_amd64.deb\`),"
    echo "or the \`.flatpak\` for any other distribution (\`flatpak install --user copper-x86_64.flatpak\`; the GNOME 51 runtime comes from Flathub)."
    echo "x86_64 and aarch64. Each file has a \`.sha256\` next to it, and \`SHA256SUMS\` lists them all."
  } >"$notes"
  # A pre-release suffix (v0.2.0-rc.1) makes a GitHub pre-release, which /latest/ ignores.
  case "$version" in *-*) flags=(--prerelease --latest=false) ;; esac
  gh release create "$tag" \
    --repo "$REPO" \
    --verify-tag \
    --title "Copper ${version} for Linux" \
    --notes-file "$notes" \
    "${flags[@]}" \
    "$dist"/*

  # Verify the published bytes.
  local base="https://github.com/${REPO}/releases/download/${tag}" f want have got
  got=$(mktemp -d)
  for f in copper_amd64.deb copper_arm64.deb copper-x86_64.flatpak copper-aarch64.flatpak copper-version.json install.sh SHA256SUMS; do
    curl -fsSL --retry 5 --retry-delay 3 -o "$got/$f" "$base/$f"
    want=$(sha256sum "$dist/$f" | cut -d' ' -f1)
    have=$(sha256sum "$got/$f" | cut -d' ' -f1)
    [ "$want" = "$have" ] || die "$base/$f serves $have, expected $want"
    echo "OK $base/$f"
  done
}

# ---------------------------------------------------------------- in the containers

install_rust() {
  local triple sum
  case "$(uname -m)" in
    x86_64) triple=x86_64-unknown-linux-gnu sum=$RUSTUP_SHA256_X86_64 ;;
    aarch64) triple=aarch64-unknown-linux-gnu sum=$RUSTUP_SHA256_AARCH64 ;;
    *) die "unsupported machine $(uname -m)" ;;
  esac
  curl -fsSL --retry 3 -o /tmp/rustup-init "https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/$triple/rustup-init"
  echo "$sum  /tmp/rustup-init" | sha256sum -c --quiet -
  chmod +x /tmp/rustup-init
  /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain none
  export PATH="$HOME/.cargo/bin:$PATH"
  rustup toolchain install # rust-toolchain.toml
}

# _deb <version> <amd64|arm64>: the .deb, lintian, install/start/remove, stable name, checksums.
_deb() {
  local version=$1 arch=$2 deb name out versioned f
  export DEBIAN_FRONTEND=noninteractive CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0
  apt-get update -q
  # shellcheck disable=SC2086 # the package list splits on purpose
  apt-get install -y -q --no-install-recommends ca-certificates curl git $APT_BUILD_DEPS dpkg-dev lintian
  # build-deb.sh dates the Debian changelog by the last commit.
  git config --global --add safe.directory /src
  install_rust
  cargo install --locked cargo-deb --version "$CARGO_DEB_VERSION"

  deb=$(packaging/build-deb.sh --locked --version "$version" --out dist)
  name=$(basename "$deb")
  dpkg-deb -I "$deb"
  dpkg-deb -c "$deb"
  lintian --fail-on error --suppress-tags no-manual-page,initial-upload-closes-no-bugs "dist/$name"

  # Install, start, remove. (No AppArmor in a container: the profile must be installed; that the
  # kernel loaded it is only checked on a real Ubuntu host.)
  apt-get install -y "./dist/$name"
  dpkg -L copper
  out=$(timeout 30 copper --version)
  echo "$out"
  [ "$out" = "copper $version" ] || die "copper --version said '$out', expected 'copper $version'"
  [ -f /etc/apparmor.d/copper ] || die "/etc/apparmor.d/copper is not installed"
  update-alternatives --query x-www-browser | grep -q /usr/bin/copper
  apt-get purge -y copper
  if command -v copper; then die "copper still installed after purge"; fi

  # Release names use the Cargo version: cargo-deb's file name carries the Debian one
  # (0.1.0~rc.1-1), and GitHub rewrites "~" in asset names.
  cd dist
  versioned="copper_${version}_${arch}.deb"
  [ "$name" = "$versioned" ] || mv "$name" "$versioned"
  cp "$versioned" "copper_${arch}.deb"
  for f in "$versioned" "copper_${arch}.deb"; do sha256sum "$f" >"$f.sha256"; done
  ls -la
}

# _flatpak <version> <x86_64|aarch64>: the bundle, install/start/remove, stable name, checksums.
_flatpak() {
  local version=$1 arch=$2 out f
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -q
  # elfutils (eu-strip) and librsvg2-common (the SVG loader appstreamcli compose reads the app
  # icon with) are only recommended by flatpak-builder; without them the build stops.
  apt-get install -y -q --no-install-recommends ca-certificates curl \
    flatpak flatpak-builder elfutils librsvg2-common python3-aiohttp python3-tomlkit dbus-daemon dbus-bin
  # flatpak run talks to the system and session buses; the container has neither.
  mkdir -p /run/dbus
  dbus-daemon --system --fork
  XDG_RUNTIME_DIR=$(mktemp -d /tmp/xdg.XXXXXX)
  chmod 700 "$XDG_RUNTIME_DIR"
  export XDG_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
  dbus-daemon --session --address="$DBUS_SESSION_BUS_ADDRESS" --fork
  # aarch64 is cross-built on the x86_64 engine (FLATPAK_ARCH; bwrap cannot run under qemu-user).
  FLATPAK_ARCH=$arch packaging/flatpak/build.sh --out dist --name "copper-$version-$arch.flatpak"

  flatpak install --user -y --noninteractive "dist/copper-$version-$arch.flatpak"
  flatpak info --user --arch="$arch" --show-permissions com.copperbrowser.Copper
  out=$(timeout 300 flatpak run --user --arch="$arch" com.copperbrowser.Copper --version)
  echo "$out"
  [ "$out" = "copper $version" ] || die "copper --version said '$out', expected 'copper $version'"
  flatpak uninstall --user -y --noninteractive --arch="$arch" com.copperbrowser.Copper

  cd dist
  cp "copper-$version-$arch.flatpak" "copper-$arch.flatpak"
  for f in "copper-$version-$arch.flatpak" "copper-$arch.flatpak"; do sha256sum "$f" >"$f.sha256"; done
  ls -la
}

# _assemble <version> <tag> <commit> <published-at>: dist/parts/* → dist/publish with the
# installer, copper-version.json and SHA256SUMS.
_assemble() {
  local version=$1 tag=$2 commit=$3 published_at=$4 sums f
  local out=dist/publish
  rm -rf "$out"
  mkdir -p "$out"
  for f in deb-amd64 deb-arm64 flatpak-x86_64 flatpak-aarch64; do
    [ -d "dist/parts/$f" ] || die "dist/parts/$f is missing"
    cp dist/parts/"$f"/* "$out"/
  done
  cp install.sh "$out/install.sh"
  (cd "$out" && sha256sum install.sh >install.sh.sha256)
  python3 packaging/version-manifest.py --dist "$out" --version "$version" --tag "$tag" \
    --commit "$commit" --repo "$REPO" --published-at "$published_at" \
    >"$out/copper-version.json"
  cat "$out/copper-version.json"
  (cd "$out" && sha256sum -c ./*.sha256)

  # SHA256SUMS: every published file except the per-file .sha256 companions.
  cd "$out"
  sums=$(mktemp)
  find . -maxdepth 1 -type f ! -name '*.sha256' ! -name SHA256SUMS -printf '%f\n' |
    LC_ALL=C sort | xargs sha256sum >"$sums"
  mv "$sums" SHA256SUMS
  chmod 644 SHA256SUMS
  cat SHA256SUMS
  sha256sum -c SHA256SUMS
  for f in "copper_${version}_amd64.deb" "copper_${version}_arm64.deb" copper_amd64.deb copper_arm64.deb \
    "copper-${version}-x86_64.flatpak" "copper-${version}-aarch64.flatpak" \
    copper-x86_64.flatpak copper-aarch64.flatpak install.sh copper-version.json; do
    grep -q "  $f\$" SHA256SUMS || die "release asset $f is missing"
  done
}

case "${1:-}" in
  build) build "${2:-}" "${3:-}" ;;
  assemble) assemble "${2:-}" ;;
  publish) publish "${2:-}" ;;
  _deb | _flatpak) "$1" "${2:?}" "${3:?}" ;;
  _assemble) _assemble "${2:?}" "${3:?}" "${4:?}" "${5:?}" ;;
  *) echo "$usage" >&2; exit 2 ;;
esac

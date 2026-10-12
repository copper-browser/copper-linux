#!/usr/bin/env bash
# build-deb.sh — build the `copper` .deb for this machine's architecture.
#
#   packaging/build-deb.sh                  # cargo build --release, then the .deb
#   packaging/build-deb.sh --no-build       # package the already-built target/release/copper
#   packaging/build-deb.sh --locked --out dist/
#
# Options:
#   --no-build       skip `cargo build --release -p copper-app`
#   --locked         pass --locked to cargo build (CI and releases use it)
#   --version V      package version (default: copper-app's Cargo version)
#   --out DIR        where the .deb goes (default: target/debian)
#
# Prints the path of the .deb on the last line of stdout. Needs cargo-deb
# (`cargo install --locked cargo-deb`) and dpkg-dev (dpkg-shlibdeps).
#
# How: cargo-deb only reads [package.metadata.deb] from a Cargo.toml and the
# crate manifest stays packaging-free, so packaging/deb.toml is rendered into
# target/deb-shim/Cargo.toml (a stand-alone metadata-only package) and cargo
# deb runs against it with --no-build and CARGO_TARGET_DIR=<repo>/target.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
build=1
locked=()
version=
out="$root/target/debian"

while [[ $# -gt 0 ]]; do
  case $1 in
    --no-build) build=0 ;;
    --locked) locked=(--locked) ;;
    --version) version=${2:?--version needs a value}; shift ;;
    --out) out=${2:?--out needs a value}; shift ;;
    -h | --help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "build-deb: unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

command -v cargo-deb >/dev/null || {
  echo "build-deb: cargo-deb not found; cargo install --locked cargo-deb" >&2
  exit 1
}

cd "$root"
if [[ -z $version ]]; then
  # "path+file:///…/crates/copper-app#0.1.0" or "…#copper-app@0.1.0"
  pkgid=$(cargo pkgid -p copper-app)
  version=${pkgid##*[#@]}
fi

if [[ $build == 1 ]]; then
  cargo build --release -p copper-app --bin copper ${locked[@]+"${locked[@]}"} >&2
fi
[[ -x target/release/copper ]] || { echo "build-deb: target/release/copper is missing" >&2; exit 1; }

shim="$root/target/deb-shim"
rm -rf "$shim"
mkdir -p "$shim" "$out"
: >"$shim/lib.rs"
# @ROOT@ may contain characters sed cares about; escape & | and \.
esc_root=$(printf '%s' "$root" | sed 's/[&|\\]/\\&/g')
sed -e "s|@ROOT@|$esc_root|g" -e "s|@VERSION@|$version|g" packaging/deb.toml >"$shim/Cargo.toml"
# A one-entry Debian changelog (cargo-deb gzips it into /usr/share/doc/copper/).
# Dated by the source's last commit when there is one, so rebuilds match.
when=$(git -C "$root" log -1 --format=%ct 2>/dev/null || date +%s)
{
  printf 'copper (%s-1) unstable; urgency=medium\n\n' "${version//-/\~}"
  printf '  * Copper %s for Linux. Release notes:\n' "$version"
  printf '    https://github.com/copper-browser/copper-linux/releases\n\n'
  printf ' -- Copper <copper-browser@users.noreply.github.com>  %s\n' "$(LC_ALL=C date -u -R -d "@$when")"
} >"$shim/changelog"

# cargo-deb prints the path of the .deb it wrote on stdout.
deb=$(CARGO_TARGET_DIR="$root/target" cargo deb \
  --manifest-path "$shim/Cargo.toml" \
  --no-build \
  --output "$out" | tail -1)
[[ -f $deb ]] || { echo "build-deb: cargo deb did not report a .deb" >&2; exit 1; }
echo "$deb"

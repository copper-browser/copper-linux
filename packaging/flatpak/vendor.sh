#!/usr/bin/env bash
# vendor.sh — turn Cargo.lock into flatpak-builder sources, for an offline build.
#
#   packaging/flatpak/vendor.sh [OUT]      # default OUT: packaging/flatpak/cargo-sources.json
#
# flatpak-builder builds without network, so every crate in Cargo.lock is
# listed as a source (URL + sha256) that flatpak-builder downloads up front and
# unpacks into $CARGO_HOME/vendor; the manifest then builds with --offline.
# The generator is flatpak-builder-tools' flatpak-cargo-generator.py, pinned to
# one commit and checked against its sha256 before it runs.
#
# Rerun whenever Cargo.lock changes (CI and release.yml run it every time;
# cargo-sources.json is not committed). Needs python3 with aiohttp and tomlkit
# (Ubuntu: sudo apt install python3-aiohttp python3-tomlkit).
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$here/../.." && pwd)

rev=74697c75b630d7330e77250fc13cb5ea688d9479 # flatpak/flatpak-builder-tools master, 2026-09-30
sum=0a2db6be87d75910facef28ab46d4d6460802e8419ab850d0caa6a364d26b380
url="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$rev/cargo/flatpak-cargo-generator.py"

cache="${XDG_CACHE_HOME:-$HOME/.cache}/copper-packaging"
gen="$cache/flatpak-cargo-generator-$rev.py"
mkdir -p "$cache"
if [[ ! -f $gen ]]; then
  curl -fsSL --retry 3 -o "$gen.tmp" "$url"
  mv "$gen.tmp" "$gen"
fi
echo "$sum  $gen" | sha256sum -c --quiet - || {
  rm -f "$gen"
  echo "vendor: flatpak-cargo-generator.py checksum mismatch" >&2
  exit 1
}

out=${1:-$here/cargo-sources.json}
python3 "$gen" "$root/Cargo.lock" -o "$out"
echo "vendor: wrote $out ($(grep -c '"type"' "$out") sources)"

#!/usr/bin/env bash
# build.sh — build the Copper Flatpak for this machine's architecture and
# export it as a single-file bundle.
#
#   packaging/flatpak/build.sh                     # → target/flatpak/copper-<arch>.flatpak
#   packaging/flatpak/build.sh --out dist/ --name copper-0.1.0-x86_64.flatpak
#   packaging/flatpak/build.sh --install           # also install it for this user
#
# Needs flatpak and flatpak-builder (apt install flatpak flatpak-builder), plus
# python3-aiohttp and python3-tomlkit for vendor.sh. The GNOME 51 SDK and the
# rust-stable extension are installed from Flathub for this user on first run
# (~3 GB). All state lives under target/flatpak/.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$here/../.." && pwd)
id=com.copperbrowser.Copper
arch=$(flatpak --default-arch)
out="$root/target/flatpak"
name=
install=0

while [[ $# -gt 0 ]]; do
  case $1 in
    --out) out=${2:?}; shift ;;
    --name) name=${2:?}; shift ;;
    --install) install=1 ;;
    -h | --help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "flatpak/build: unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
name=${name:-copper-$arch.flatpak}
work="$root/target/flatpak"
mkdir -p "$work" "$out"

flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo

# Build from a copy of the manifest under target/ (with the source dir made
# absolute) next to its own cargo-sources.json, so nothing generated lives in
# the source tree — a `bin/dbx` sync from another checkout cannot delete it
# mid-build.
"$here/vendor.sh" "$work/cargo-sources.json"
sed "s|^        path: \.\./\.\.$|        path: $root|" "$here/$id.yml" >"$work/$id.yml"
grep -q "^        path: $root$" "$work/$id.yml" || { echo "flatpak/build: could not point the manifest at $root" >&2; exit 1; }

flatpak-builder \
  --user \
  --install-deps-from=flathub \
  --force-clean \
  --disable-rofiles-fuse \
  --state-dir="$work/state" \
  --repo="$work/repo" \
  "$work/build" \
  "$work/$id.yml"

flatpak build-bundle \
  --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
  "$work/repo" "$out/$name" "$id"

if [[ $install == 1 ]]; then
  # A bundle does not install over an installed copy, even with --reinstall.
  flatpak uninstall --user -y --noninteractive "$id" >/dev/null 2>&1 || true
  flatpak install --user -y --noninteractive "$out/$name"
fi
echo "$out/$name"

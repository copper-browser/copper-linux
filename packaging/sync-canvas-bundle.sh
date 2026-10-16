#!/usr/bin/env bash
# sync-canvas-bundle.sh — copy the shared canvas page from a macOS Copper
# checkout into this repo (one command per re-sync).
#
#   packaging/sync-canvas-bundle.sh <mac-checkout>            # its working tree
#   packaging/sync-canvas-bundle.sh <mac-checkout> <git-ref>  # e.g. origin/fork
#   packaging/sync-canvas-bundle.sh --check <mac-checkout> [<git-ref>]
#
# The board page is built once, in the macOS repo (`Canvas/` → bun →
# `Canvas/dist/canvas.html`), and shipped by the Mac app as
# `Sources/Search/Fork/Canvas/Resources/canvas.html`. Linux ships the same
# bytes as `crates/copper-app/src/canvas/resources/canvas.html` (include_str!),
# with the "Switching from Chrome" guide beside it. This copies both, prints
# what changed, and refuses anything that is not a canvas bundle. Nothing in
# the macOS checkout is written; with a ref, only `git show` reads it.
#
# --check: compare only; exit 1 when the copies differ.
set -euo pipefail

check=0
if [[ ${1:-} == --check ]]; then
  check=1
  shift
fi
if [[ $# -lt 1 || $# -gt 2 ]]; then
  sed -n '2,17p' "$0" >&2
  exit 2
fi
mac=$1
ref=${2:-}
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
dest="$root/crates/copper-app/src/canvas/resources"
files=(canvas.html switching-from-chrome.json)
src_dir=Sources/Search/Fork/Canvas/Resources

[[ -d $mac ]] || { echo "not a directory: $mac" >&2; exit 2; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for f in "${files[@]}"; do
  if [[ -n $ref ]]; then
    git -C "$mac" show "$ref:$src_dir/$f" >"$tmp/$f"
  else
    cp "$mac/$src_dir/$f" "$tmp/$f"
  fi
done

# A canvas bundle installs window.copperCanvas and posts to the canvas handler.
for marker in copperCanvas messageHandlers; do
  if ! grep -q "$marker" "$tmp/canvas.html"; then
    echo "$src_dir/canvas.html has no '$marker': not a canvas bundle" >&2
    exit 1
  fi
done
python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$tmp/switching-from-chrome.json"

sha() { shasum -a 256 "$1" | cut -c1-16; }
differs=0
for f in "${files[@]}"; do
  old="$dest/$f"
  if [[ -f $old ]] && cmp -s "$old" "$tmp/$f"; then
    echo "$f: unchanged ($(sha "$old"), $(wc -c <"$old" | tr -d ' ') bytes)"
    continue
  fi
  differs=1
  before=$([[ -f $old ]] && echo "$(sha "$old"), $(wc -c <"$old" | tr -d ' ') bytes" || echo none)
  echo "$f: $before -> $(sha "$tmp/$f"), $(wc -c <"$tmp/$f" | tr -d ' ') bytes"
  if [[ $check -eq 0 ]]; then
    cp "$tmp/$f" "$old"
  fi
done

from=${ref:-working tree}
if [[ -n $ref ]]; then
  from="$ref ($(git -C "$mac" rev-parse --short "$ref"))"
fi
if [[ $check -eq 1 ]]; then
  echo "compared with $mac @ $from"
  exit "$differs"
fi
echo "synced from $mac @ $from"

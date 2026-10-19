#!/usr/bin/env bash
# sync-canvas-bundle.sh — copy the shared canvas page into this repo (one command per re-sync).
#
#   packaging/sync-canvas-bundle.sh --canvas-app <ref>        # the page from copper-browser/canvas-app
#   packaging/sync-canvas-bundle.sh <mac-checkout>            # page + guide from a macOS checkout's working tree
#   packaging/sync-canvas-bundle.sh <mac-checkout> <git-ref>  # e.g. origin/fork
#   packaging/sync-canvas-bundle.sh --check … (either form)   # compare only; exit 1 when the copies differ
#
# The board page lives in copper-browser/canvas-app, the one source for Copper on every
# platform; its embed build is `dist/canvas.html`. Linux ships those bytes as
# `crates/copper-shell-core/src/canvas/resources/canvas.html` (include_str!), and records the
# canvas-app commit in `CANVAS_APP_REF` beside it. --canvas-app fetches that build (a ref is
# resolved to a commit) and pins it. The macOS form also copies the "Switching from Chrome"
# guide, which still lives in the Mac app (`Sources/Search/Fork/Canvas/Resources/`); nothing
# in the macOS checkout is written, and with a ref only `git show` reads it. Either way it
# prints what changed and refuses anything that is not a canvas bundle.
set -euo pipefail

check=0
if [[ ${1:-} == --check ]]; then
  check=1
  shift
fi
if [[ ${1:-} == --canvas-app ]]; then
  [[ $# -eq 2 ]] || { sed -n '2,16p' "$0" >&2; exit 2; }
  root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
  dest="$root/crates/copper-shell-core/src/canvas/resources"
  repo=copper-browser/canvas-app
  sha=$(git ls-remote "https://github.com/$repo.git" "$2" | head -1 | cut -f1)
  [[ $2 =~ ^[0-9a-f]{40}$ ]] && sha=$2
  [[ -n $sha ]] || { echo "no canvas-app ref $2" >&2; exit 2; }
  tmp=$(mktemp)
  trap 'rm -f "$tmp"' EXIT
  curl -fsSL "https://raw.githubusercontent.com/$repo/$sha/dist/canvas.html" -o "$tmp"
  for marker in copperCanvas messageHandlers; do
    grep -q "$marker" "$tmp" || { echo "canvas-app $sha dist/canvas.html has no '$marker': not a canvas bundle" >&2; exit 1; }
  done
  if cmp -s "$tmp" "$dest/canvas.html"; then
    echo "canvas.html: unchanged (canvas-app $sha)"
    [[ $check -eq 1 ]] || echo "$sha" >"$dest/CANVAS_APP_REF"
    exit 0
  fi
  echo "canvas.html: $(wc -c <"$dest/canvas.html" | tr -d ' ') -> $(wc -c <"$tmp" | tr -d ' ') bytes (canvas-app $sha)"
  [[ $check -eq 1 ]] && exit 1
  cp "$tmp" "$dest/canvas.html"
  echo "$sha" >"$dest/CANVAS_APP_REF"
  echo "synced from canvas-app $sha"
  exit 0
fi
if [[ $# -lt 1 || $# -gt 2 ]]; then
  sed -n '2,16p' "$0" >&2
  exit 2
fi
mac=$1
ref=${2:-}
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
dest="$root/crates/copper-shell-core/src/canvas/resources"
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

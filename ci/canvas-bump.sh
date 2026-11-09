#!/usr/bin/env bash
# canvas-bump.sh — hourly (or by hand) on the Roadrunner host (.woodpecker/canvas.yaml): when
# copper-browser/canvas-app main has moved, push the pinned branch canvas/take-canvas-app and open
# or update one issue with its commit list and a link that opens the PR; close the issue once the
# pin has caught up. Needs git, curl, jq and GH_TOKEN (contents + issues write on this repo).
# Runs on a checkout of main.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${GH_TOKEN:?GH_TOKEN must be set}"

repo=${CI_REPO:-copper-browser/copper-linux}
branch=canvas/take-canvas-app
title="Canvas: canvas-app moved on"
ref=crates/copper-shell-core/src/canvas/resources/CANVAS_APP_REF
export GIT_TERMINAL_PROMPT=0

api() {
  curl -fsSL --retry 3 -H "Authorization: Bearer $GH_TOKEN" -H "Accept: application/vnd.github+json" \
    -H "X-GitHub-Api-Version: 2022-11-28" "$@"
}
# Open issues (not pull requests) titled exactly $title.
open_issues() {
  api "https://api.github.com/repos/$repo/issues?state=open&per_page=100" |
    jq -r --arg t "$title" '.[] | select(.pull_request == null and .title == $t) | .number'
}

# Pin canvas-app main.
old=$(cat "$ref")
packaging/sync-canvas-bundle.sh --canvas-app main
new=$(cat "$ref")

# Only the page counts: a canvas-app commit that leaves dist/canvas.html as it was keeps the pin.
if git diff --quiet -- crates/copper-shell-core/src/canvas/resources/canvas.html; then
  git checkout -- "$ref"
  for num in $(open_issues); do
    api -X POST "https://api.github.com/repos/$repo/issues/$num/comments" -o /dev/null \
      -d "$(jq -n --arg b "Linux pins canvas-app $(cut -c1-7 "$ref"), its main." '{body: $b}')"
    api -X PATCH "https://api.github.com/repos/$repo/issues/$num" -o /dev/null -d '{"state":"closed"}'
    echo "closed #$num"
  done
  exit 0
fi

# Push the pinned branch (token only in an http header, never in the URL or the remote).
git checkout -B "$branch"
git -c user.name=exowatt-woodpecker-bot \
  -c user.email=339688546+exowatt-woodpecker-bot@users.noreply.github.com \
  commit -qam "canvas: take canvas-app ${new:0:7}" \
  -m "https://github.com/copper-browser/canvas-app/compare/${old:0:12}...${new:0:12}"
auth=$(printf 'x-access-token:%s' "$GH_TOKEN" | base64 -w0)
git -c http.extraheader="Authorization: Basic $auth" \
  push -f "https://github.com/$repo.git" "HEAD:refs/heads/$branch"

# Open or update the issue.
log=$(api "https://api.github.com/repos/copper-browser/canvas-app/compare/$old...$new" |
  jq -r '.commits[] | "- " + (.commit.message | split("\n")[0]) + " (" + .sha[0:7] + ")"' | head -50)
# shellcheck disable=SC2016 # the backticks are Markdown
body=$(printf 'canvas-app `main` is at %s; Linux pins %s. Branch `%s` pins it and vendors its `dist/canvas.html` (`packaging/sync-canvas-bundle.sh --canvas-app`). Merge after its CI (the canvas webview tests) passes.\n\n**[Open the pull request](https://github.com/%s/compare/main...%s?expand=1)**\n\n%s\n\nhttps://github.com/copper-browser/canvas-app/compare/%s...%s\n\nThis issue is updated every hour while the pin is behind; it closes when the pin catches up.\n' "${new:0:7}" "${old:0:7}" "$branch" "$repo" "$branch" "$log" "${old:0:12}" "${new:0:12}")
num=$(open_issues | head -1)
if [ -n "$num" ]; then
  api -X PATCH "https://api.github.com/repos/$repo/issues/$num" -o /dev/null -d "$(jq -n --arg b "$body" '{body: $b}')"
  echo "updated #$num"
else
  api -X POST "https://api.github.com/repos/$repo/issues" -o /dev/null -d "$(jq -n --arg t "$title" --arg b "$body" '{title: $t, body: $b}')"
  echo "opened an issue"
fi

#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

echo "Building docs ..."
mdbook build

echo "Deploying to gh-pages ..."
TMPDIR=$(mktemp -d)
git worktree add "$TMPDIR" gh-pages
cp -r book/. "$TMPDIR/"
cd "$TMPDIR"
git add -A
git commit -m "docs: update gh-pages" --allow-empty
git push public gh-pages
cd -
git worktree remove "$TMPDIR"

echo "Done. https://deeper4ai.github.io/ramparils/"

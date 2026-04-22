#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$REPO_ROOT"

echo "Building docs ..."
mdbook build

echo "Deploying to gh-pages ..."
# Push book/ to gh-pages branch on the public remote
TMPDIR=$(mktemp -d)
git worktree add "$TMPDIR" gh-pages
cp -r book/. "$TMPDIR/"
cd "$TMPDIR"
git add -A
git commit -m "docs: update gh-pages" --allow-empty
git push public gh-pages
cd "$REPO_ROOT"
git worktree remove "$TMPDIR"

echo "Done. https://deeper4ai.github.io/ramparils/"

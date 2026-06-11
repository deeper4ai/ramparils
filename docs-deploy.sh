#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")" && pwd)
cd "$ROOT"
REMOTE=${DOCS_REMOTE:-origin}
BRANCH=${DOCS_BRANCH:-gh-pages}

if [[ -n $(git status --porcelain --untracked-files=normal) ]]; then
    echo "Refusing to publish from a dirty worktree." >&2
    echo "Commit or stash source changes first." >&2
    exit 1
fi

SOURCE_COMMIT=$(git rev-parse HEAD)

echo "Building docs ..."
mdbook build

echo "Deploying to gh-pages ..."
TMPDIR=$(mktemp -d)
cleanup() {
    cd "$ROOT"
    git worktree remove --force "$TMPDIR" >/dev/null 2>&1 || true
    rm -rf "$TMPDIR"
}
trap cleanup EXIT

git fetch "$REMOTE" "$BRANCH"
git worktree add --detach "$TMPDIR" "$REMOTE/$BRANCH"
git -C "$TMPDIR" rm -rq --ignore-unmatch .
cp -r book/. "$TMPDIR/"
git -C "$TMPDIR" add -A
git -C "$TMPDIR" commit \
    -m "docs: update gh-pages" \
    -m "Source: $SOURCE_COMMIT" \
    --allow-empty
git -C "$TMPDIR" push "$REMOTE" "HEAD:$BRANCH"

echo "Done. https://deeper4ai.github.io/ramparils/"

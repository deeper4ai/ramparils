#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"

if ! command -v twine &>/dev/null; then
    echo "twine not found. Install it with: pipx install twine"
    exit 1
fi

echo "Uploading to PyPI ..."
twine upload "$REPO_ROOT/target/wheels/"*.whl

echo "Done."

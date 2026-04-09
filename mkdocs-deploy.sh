#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
VENV="$REPO_ROOT/.venv"

if [ ! -d "$VENV" ]; then
    echo "Creating virtualenv at .venv ..."
    python -m venv "$VENV"
fi

source "$VENV/bin/activate"

echo "Installing/updating dependencies ..."
pip install -q maturin mkdocs mkdocs-material "mkdocstrings[python]"

echo "Building Python extension ..."
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 maturin develop --features python -q

echo "Deploying docs to gh-pages ..."
mkdocs gh-deploy --remote-name public

echo "Done. https://deeper4ai.github.io/parils/"

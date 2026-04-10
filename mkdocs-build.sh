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
maturin develop --features python -q

echo "Building docs ..."
mkdocs build

echo "Done. Site in site/"

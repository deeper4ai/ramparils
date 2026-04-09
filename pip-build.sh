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
pip install -q maturin ziglang

echo "Building manylinux wheel (abi3, x86_64) ..."
maturin build --release --features python --zig -i python

echo "Done. Wheel in target/wheels/"
ls "$REPO_ROOT/target/wheels/"

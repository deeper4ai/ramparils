#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
echo "Serving docs at http://localhost:3000 ..."
mdbook serve --open

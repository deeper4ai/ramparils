#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
mdbook build
echo "Done. Site in book/"

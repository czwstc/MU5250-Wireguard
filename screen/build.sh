#!/bin/sh
# Reproducible DevUI/litehtml renderer with the existing Atomic display supervisor.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
exec python3 "$root/screen/devui/build.py" "$@"

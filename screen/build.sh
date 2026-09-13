#!/bin/sh
# Isolated compiler: set ZIG to a Zig 0.14.1 executable. No host installation needed.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
python3 screen/font.py build/screen
if [ "${1:-}" = "--preview" ]; then
    cc -std=c11 -D_POSIX_C_SOURCE=200809L -DSCREEN_PREVIEW -Wall -Wextra -Werror -Wno-unused-function -Ibuild/screen screen/panel.c -o build/screen/preview
    build/screen/preview build/screen
else
    "${ZIG:-zig}" cc -target aarch64-linux-musl -std=c11 -static -Os -Wall -Wextra -Werror -Wno-unused-function -Ibuild/screen screen/panel.c -o build/screen/openui-screen
fi

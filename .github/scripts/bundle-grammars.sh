#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "Usage: $0 <grammar sources directory> <output.tar.xz>" >&2
    exit 1
fi

archive="$(CDPATH='' cd -- "$(dirname -- "$2")" && pwd)/$(basename -- "$2")"
CDPATH='' cd -- "$1"
# The compiler needs regular source files, not upstream editor integration
# symlinks. Some point to missing binaries or their own directory, which Git
# Bash cannot extract on Windows. Do not follow them or ship Git metadata.
find . -name .git -prune -o -type f -print0 |
    tar --null --no-recursion -cJf "$archive" --files-from -

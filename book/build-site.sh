#!/bin/sh

set -eu

book_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
exec python3 "$book_dir/build-site.py"

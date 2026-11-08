#!/usr/bin/env bash
set -euo pipefail

image=$(realpath "$1")
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
# Uploaded artifacts lose executable bits. Run away from the source tree and
# installed runtimes so the health check must use the image's own assets.
chmod +x "$image"
cd "$work"
unset CARGO_MANIFEST_DIR MITOS_RUNTIME
export XDG_CONFIG_HOME="$work/config" XDG_DATA_HOME="$work/data" XDG_STATE_HOME="$work/state"
export APPIMAGE_EXTRACT_AND_RUN=1 NO_COLOR=1
"$image" --version
"$image" --health rust | tee health.txt
grep -q 'Tree-sitter parser:.*✓' health.txt
grep -q 'Highlight queries:.*✓' health.txt

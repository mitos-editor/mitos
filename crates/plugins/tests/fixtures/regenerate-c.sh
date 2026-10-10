#!/bin/sh
set -eu
fixture_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH= cd -- "$fixture_dir/../../../.." && pwd)
bindings_dir=$(mktemp -d "${TMPDIR:-/tmp}/mitos-c-fixture.XXXXXX")
trap 'rm -rf "$bindings_dir"' EXIT HUP INT TERM
if [ -n "${MITOS_C_BINDINGS:-}" ]; then
    "$MITOS_C_BINDINGS" "$repository/crates/plugin-api/wit" "$bindings_dir"
else
    cargo run --manifest-path "$repository/tools/plugin-pack/Cargo.toml" --locked "$@" --bin c-bindings -- "$repository/crates/plugin-api/wit" "$bindings_dir"
fi
clang --target=wasm32 -O2 -ffreestanding -fno-builtin -I "$fixture_dir/c-guest/include" -c "$bindings_dir/plugin.c" -o "$bindings_dir/plugin.o"
clang --target=wasm32 -O2 -ffreestanding -fno-builtin -I "$fixture_dir/c-guest/include" -I "$bindings_dir" -c "$fixture_dir/c-guest/guest.c" -o "$bindings_dir/guest.o"
wasm-ld --no-entry --export-memory --initial-memory=131072 --max-memory=67108864 "$bindings_dir/plugin.o" "$bindings_dir/guest.o" "$bindings_dir/plugin_component_type.o" -o "$fixture_dir/c-guest.wasm"

chmod 644 "$fixture_dir/c-guest.wasm"

#!/bin/sh
set -eu
fixture_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fixture_target=${MITOS_FIXTURE_TARGET_DIR:-"$fixture_dir/target"}
cargo build --manifest-path "$fixture_dir/component-guest/Cargo.toml" --target-dir "$fixture_target" --target wasm32-unknown-unknown --release --locked "$@"
cp "$fixture_target/wasm32-unknown-unknown/release/mitos_component_test_guest.wasm" "$fixture_dir/component-guest.wasm"
cargo build --manifest-path "$fixture_dir/sdk-guest/Cargo.toml" --target-dir "$fixture_target" --target wasm32-unknown-unknown --release --locked "$@"
cp "$fixture_target/wasm32-unknown-unknown/release/mitos_sdk_test_guest.wasm" "$fixture_dir/sdk-guest.wasm"
cargo build --manifest-path "$fixture_dir/router-guest/Cargo.toml" --target-dir "$fixture_target" --target wasm32-unknown-unknown --release --locked "$@"
cp "$fixture_target/wasm32-unknown-unknown/release/mitos_router_test_guest.wasm" "$fixture_dir/router-guest.wasm"
cargo run --manifest-path "$fixture_dir/../../../../tools/plugin-pack/Cargo.toml" --locked "$@" -- "$fixture_dir/router-guest.wasm" "$fixture_dir/router-guest.component.wasm"
cargo build --manifest-path "$fixture_dir/workflow-guest/Cargo.toml" --target-dir "$fixture_target" --target wasm32-unknown-unknown --release --locked "$@"
cargo run --manifest-path "$fixture_dir/../../../../tools/plugin-pack/Cargo.toml" --locked "$@" -- "$fixture_target/wasm32-unknown-unknown/release/mitos_workflow_test_guest.wasm" "$fixture_dir/workflows.component.wasm"

chmod 644 "$fixture_dir"/*.wasm

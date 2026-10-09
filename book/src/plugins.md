# WebAssembly plugins

Mitos plugins are core WebAssembly modules. They run in Wasmi and receive an
owned snapshot of the editor through a JSON protocol. A plugin can edit the
document, change selections, open a path, and show status or error messages.
Plugins can be written in any language that implements the memory ABI; a Rust
SDK and an example are included in the source repository.

## Installing and configuring a plugin

A plugin consists of a `plugin.toml` manifest and a `.wasm` module. Put both in
the same directory, for example `~/.config/mitos/plugins/uppercase`, and add an
entry to your `config.toml`:

```toml
[plugins.uppercase]
path = "plugins/uppercase/plugin.toml"
enabled = true
config = {}

[keys.normal]
U = ":uppercase.uppercase"
```

`path` points to the manifest; the containing directory is also accepted.
Relative paths resolve from the standard Mitos configuration directory, including
when a custom `--config` file is used. `enabled`
defaults to `true`. The optional `config` table is passed to the plugin on each
invocation; its keys and values are defined by that plugin.

The name following `[plugins.]` is the plugin's ID. Its manifest command names
are qualified with that ID: the `uppercase` command from `[plugins.uppercase]`
becomes `:uppercase.uppercase`. Plugin commands appear in command completion and
the command palette, and work in key bindings and custom commands. Arguments
use the editor's existing command parsing, including quoting and expansions.

Apply configuration changes with `:config-reload`. After rebuilding a module,
use `:plugin-reload` to load the new binary. Failed plugins report an error while
the editor and other plugins continue running. A trapped plugin is disabled
until it is reloaded.

Trusted workspace configuration can override plugin entries by their configured
name, following the existing [workspace trust](./workspace-trust.md) policy.
Plugins are loaded only from explicit configuration; Mitos does not discover or
automatically run modules from a workspace directory.

## Plugin manifests

This is the manifest for the example plugin:

```toml
abi-version = 2
module = "uppercase.wasm"

[commands.uppercase]
doc = "Uppercase each selection, preserving Unicode and multiple selections."
```

`module` resolves relative to the manifest and must remain inside its directory.
Each command declares the documentation shown in completion. A plugin can also
subscribe to editor events with a top-level `events` list, before the command
tables:

```toml
abi-version = 2
module = "my-plugin.wasm"
events = ["document-saved", "selection-changed"]

[commands.example]
doc = "Run the example command."
```

Available hooks are `document-opened`, `document-changed`, `document-saved`,
`document-closed`, `selection-changed`, `mode-changed`, and `post-command`.
`document-saved` runs after a successful write. Every plugin receives `init`
and `shutdown`; these lifecycle events do not need to be listed. `command`
invocations are dispatched from the manifest's command declarations.

## Writing a Rust plugin

The source repository's `crates/plugin-sdk` crate supplies request and response
types and an `export_plugin!` macro. Compile a `cdylib` for
`wasm32-unknown-unknown`; WASI modules are not supported.

```rust
use plugin_sdk::{Action, Event, Request, Response, export_plugin};

fn handle(request: Request) -> Response {
    if request.event == Event::Command {
        return Response {
            actions: vec![Action::Status {
                message: "Hello from WebAssembly".into(),
            }],
            error: None,
        };
    }
    Response::default()
}

export_plugin!(handle);
```

For a complete working example, see `examples/plugins/uppercase` in the
repository. It transforms all selections and preserves their directions and
the primary selection, including Unicode uppercase expansion such as `ß` to
`SS`. From the repository root, build and install it with:

```sh
rustup target add wasm32-unknown-unknown
cargo build --manifest-path examples/plugins/uppercase/Cargo.toml \
  --target wasm32-unknown-unknown --release --locked
mkdir -p ~/.config/mitos/plugins/uppercase
cp examples/plugins/uppercase/plugin.toml ~/.config/mitos/plugins/uppercase/
cp examples/plugins/uppercase/target/wasm32-unknown-unknown/release/uppercase.wasm \
  ~/.config/mitos/plugins/uppercase/
```

Add the configuration above, reload it, select text, and run
`:uppercase.uppercase`.

## Protocol and limits

The ABI exports linear `memory`, `mitos_alloc(i32) -> i32`,
`mitos_dealloc(i32, i32)`, and `mitos_call(i32, i32) -> i64`. Request and response
buffers hold UTF-8 JSON. The response packs its address into the upper 32 bits
and its byte length into the lower 32 bits. The host owns and frees both buffers;
the plugin only reads the input and allocates the output. The SDK implements
this contract for Rust guests. Its README documents the full JSON schema for
other languages.

Document offsets count Unicode scalar values, matching Mitos's text coordinates.
An edit replaces the half-open range `start..end` and includes the original
document ID and version. Edits from stale snapshots, overlapping ranges, and
invalid selections are rejected. Edits within one action use its preceding
projected text; later actions use the text after earlier edits, while retaining
the original version precondition. Selection actions include the originating
view's ID, binding revision, and selection revision. A changed selection, closed
view, or view switched away and back rejects the batch before mutation. Both
edit and selection actions must precede any open action in the response,
because opening a path can replace their view. Snapshots can omit the current
document, path, or language; handlers must account for these cases.

Document-only edits can target hidden documents and compose into one undo step.
Selection changes require their explicit live view; the host does not substitute
the focused split. Plugin actions currently suppress hooks across all plugins, so a
document-change hook can return an edit without recursively invoking plugins.
Pending document-change events coalesce per document and selection events per view;
the queue retains at most 32 snapshots. Hooks can observe the latest state but
are not guaranteed to receive every intermediate change.

The initial API provides synchronous commands and hooks. It has no WASI or
ambient filesystem/network access, arbitrary editor-command execution, custom
UI, LSP calls, Tree-sitter handles, or asynchronous jobs. Opening a path is an
explicit editor action. Each instance has a 64 MiB memory limit and each
invocation has a 10 million fuel budget. Document snapshots are limited to 2 MiB
of text. JSON requests and responses are limited to 4 MiB and responses to 256
actions. Modules are limited to 16 MiB, manifests
to 64 KiB, and Wasm tables to 4096 elements. A document that exceeds the snapshot
limit cannot be delivered to a plugin, and heavily escaped JSON can reach the
message limit even with a smaller document.

This integration adapts the lifecycle, command registration, and event-hook
boundaries of [Helix PR #8675](https://github.com/helix-editor/helix/pull/8675),
reviewed at revision `c16fac096a9dd162d46f53bf2411f36251d755f3`, to Mitos's current
editor structure. Its Steel runtime and bindings are replaced with the
WebAssembly protocol described here.

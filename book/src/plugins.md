# WebAssembly plugins

Mitos plugins are WebAssembly components with a versioned WIT interface.
They run in a lean Wasmtime host on dedicated workers and receive document/view
metadata with explicit revisions. Text is read through bounded host services;
status commands do not copy the current document. The Rust SDK supplies generated
bindings, owned request types, and an `export_plugin!` macro. Other languages can
generate bindings from `crates/plugin-api/wit/plugin.wit`.

The API is experimental. The old core-WASM allocator/JSON ABI is retired;
rebuild prototype plugins with the component SDK.

## Installing and configuring a plugin

A plugin consists of a `plugin.toml` manifest and a `.wasm` module. Put both in
the same directory, for example `~/.config/mitos/plugins/uppercase`, and add an
entry to your `config.toml`:

```toml
[plugins.uppercase]
path = "plugins/uppercase/plugin.toml"
enabled = true
config = {}

[plugins.uppercase.permissions]
capabilities = ["editor-read", "editor-edit", "editor-selection", "ui"]

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
Commands can declare bounded positional counts and static literal completions;
completion never executes the guest or reads the filesystem.

Apply configuration changes with `:config-reload`. After rebuilding a module,
use `:plugin-reload` to load the new binary. Failed plugins report an error while
the editor and other plugins continue running. A trapped plugin is disabled
until it is reloaded.

Trusted workspace configuration can override plugin entries by their configured
name, following the existing [workspace trust](./workspace-trust.md) policy.
Only global user configuration grants permissions. Workspace overrides cannot
expand grants or move them to a different package path. See
[plugin permissions](./plugin-permissions.md) for filesystem roots, digest pins,
native execution, and resource limits.
Plugins are loaded only from explicit configuration; Mitos does not discover or
automatically run modules from a workspace directory.

## Plugin manifests

This is the manifest for the example plugin:

```toml
abi-version = 3
module = "uppercase.component.wasm"
capabilities = ["editor-read", "editor-edit", "editor-selection", "ui"]

[commands.uppercase]
doc = "Uppercase each selection, preserving Unicode and multiple selections."
```

`module` resolves relative to the manifest and must remain inside its directory.
Each command declares the documentation shown in completion. A plugin can also
subscribe to editor events with a top-level `events` list, before the command
tables:

```toml
abi-version = 3
module = "my-plugin.component.wasm"
events = ["document-saved", "selection-changed"]

[commands.example]
doc = "Run the example command."
```

Available hooks are `document-opened`, `document-changed`, `document-saved`,
`document-closed`, `selection-changed`, `mode-changed`, `post-command`,
`post-insert-char`, `document-focus-lost`, `terminal-focus-gained`, and
`terminal-focus-lost`.
`document-saved` runs after a successful write. Every plugin receives `init`
and `shutdown`; these lifecycle events do not need to be listed. `command`
invocations are dispatched from the manifest's command declarations.

## Writing a Rust plugin

The source repository's `crates/plugin-sdk` crate supplies request and response
types and an `export_plugin!` macro. Compile a `cdylib` for
`wasm32-unknown-unknown`, then package its embedded interface metadata into a
component. This world imports no WASI interfaces or ambient filesystem services.

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
cargo run --manifest-path tools/plugin-pack/Cargo.toml --locked -- \
  examples/plugins/uppercase/target/wasm32-unknown-unknown/release/uppercase.wasm \
  ~/.config/mitos/plugins/uppercase/uppercase.component.wasm
```

Add the configuration above, reload it, select text, and run
`:uppercase.uppercase`.

## Protocol and limits

The component exports the typed `handle` function from `mitos:plugin@0.1.0`.
Generated canonical bindings lift the request and call capability-checked imports.
Edits and other effects stream through bounded resources; finishing a resource
stages its contents. The host applies effects only after successful handler
completion and full editor preflight. Traps and rejected batches discard staged
edits. Configuration and event-specific data remain bounded JSON inside the
versioned interface. Packages supply source components; guest-provided native
compiled artifacts are never accepted.

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
the focused split. A plugin does not receive its own mutation echoes; other
plugins can observe those changes. Event `data.provenance` includes the host
generation, sequence, parent sequence, originating plugin, and causal depth.
Feedback is limited to eight causal generations.

Pending document-change events coalesce per document and selection events per
view. The queue reserves 32 data and 32 control entries, with an aggregate 8 MiB
limit. Overflow or a causal cutoff emits `resync-required`, including the lost
sequence range. `Action::RequestState` requests a catalog page or a selected
document/view; the requesting plugin receives a targeted `state` event. These
two recovery events are delivered without a manifest subscription. Catalog
pages contain at most 64 documents and views with exclusive continuation cursors.

Saved events name the path and revision actually written, even when newer edits
exist. Both ordinary writes and headless flushes use the same completion path.
Shutdown drains accepted callbacks and writes before delivering saved events and
the final shutdown hook. Failed writes do not emit saved success.

Post-command metadata records the canonical command, arguments, count, register,
origin, and outcome. Success, error, or cancellation follows that invocation's
callbacks, next-key continuation, jobs, and exact submitted writes. Unrelated
later status changes do not change its outcome. Detached LSP command/stop/restart,
backend code-action-resolution dispatch, and external URL opening report
`accepted`; this outcome does not promise completion of the external operation.
Character hooks cover ordinary insertion and macro replay;
bulk paste is observed through document changes. Hooks are observations and must
validate revisions before editing in response.

Guest execution is serialized per instance on two dedicated workers. An
independent 2 ms epoch ticker provides yielding and interrupt progress; each
invocation also has a five-second deadline. Reload, target close, and shutdown
cancel obsolete work and await owned job cleanup. Typed permission/stale-state
errors preserve a healthy instance; guest traps or interruption discard its store.
Replacement packages prepare off-thread before the editor switches generations.
A failed replacement leaves the previous generation active.

The host lazily creates the engine only when executable plugins are enabled.
Source components are capped at 16 MiB. Guest memory is bounded to an aggregate
64 MiB per store and 256 MiB per worker pool across component memories. Effects
are limited to 256 actions, 4,096 entries, and 4 MiB retained bytes. Calls,
compilation, cached code, handles, jobs, and completed results have separate
budgets. Each region read is bounded to 4 MiB and requires its live text version.
One retained source snapshot is admitted per editor, including across reloads,
with a 128 MiB source limit. Oversized selections are rejected before allocation.
A saved event describes the written version; if the document has changed since
then, reading that old version returns `stale-state` rather than retaining
unbounded historical buffers. See the [runtime limits](../../crates/plugins/RUNTIME.md)
and [permission policy](./plugin-permissions.md) for the full boundary.

In-process guest limits do not promise hard editor-process RSS containment:
compilation and canonical string lifting use host allocations. Explicitly granted
native tools execute with operating-system authority outside WASM memory isolation.

This integration adapts the lifecycle, command registration, and event-hook
boundaries of [Helix PR #8675](https://github.com/helix-editor/helix/pull/8675),
reviewed at revision `c16fac096a9dd162d46f53bf2411f36251d755f3`, to Mitos's current
editor structure. Its Steel runtime and bindings are replaced with the
WebAssembly protocol described here.

## Editor services and interactive workflows

Use `component::editor_request` with the closed `EditorRequest` enum for named
scratch buffers, scoped file navigation, focus/split/close, registers, settings,
structural syntax captures, hover, and symbols. Every operation checks its own
capabilities and explicit document/view revisions. `OpenAt` counts lines and
Unicode scalar columns from zero and rejects invalid coordinates. Closing a
modified document requires a separate native user decision.

Successful mutating service replies mean the native operation already applied;
a later guest trap does not roll it back. Returned handles can be used in the
same invocation. Staged `Response` edit batches keep their separate transactional
preflight and undo behavior.

`Action::ShowUi` supplies a prompt, picker with cached previews, or bounded next-key
request. The native frontend presents it and returns a targeted `ui-result` event;
`data.response` contains its typed `UiResponse`. Job readiness instead carries
`data.job`. An instance is available for other invocations while the user decides.
Cancellation, timeout, origin loss, and reload close the owned native layer.
Scoped keymaps can invoke only the package's declared commands; user mappings
win conflicts, and unload restores surviving registrations. Builtin composition
uses a reviewed clipboard-free enum and reports partial completion on failure.

Register values are limited to 64 entries and 4 KiB in total. Clipboard registers
`*` and `+` additionally require `clipboard`. Clipboard IPC runs off the editor
thread with two shared slots, a 4 KiB text limit, and a two-second caller deadline.
Unix command providers terminate and reap their process group on failure or
cancellation. Custom providers additionally require the exact configured program
and argv in both process declarations and user grants. Native desktop clipboard
owners can intentionally persist after success. Terminal OSC52 and Windows native
IPC cannot be hard-interrupted; their slots stay charged until the OS operation
finishes. Command providers are unavailable on platforms without process-tree
cleanup; Windows native clipboard access remains available.

Settings overrides cover auto-format, soft-wrap, cursorline, and native theme
selection. Overrides belong to a package generation, retain the current native
baseline, and disappear when that owner unloads or traps. Plugins cannot change
filesystem roots, process policy, providers, or executable configuration through
settings.

Syntax queries return owned structural capture ranges from the existing grammar,
with bounded source/range/result counts. Predicates are initially unsupported.
A native query already running cannot be interrupted by the caller deadline;
one shared worker permit stays held until it finishes. Hover and symbol requests
use already attached servers and never start providers. They expire after two
seconds, send LSP cancellation, bound pending plugin requests to 64 per client,
and reject replies if the original document revision changed.

The public SDK [workflows example](https://github.com/mitos-editor/mitos/tree/main/examples/plugins/workflows)
contains a persistent recent-files picker, streamed project search with cached
previews and Unicode navigation, and a manual formatter with exact process grants.
Formatting supplies a later revisioned edit; wait for completion before saving.
Reload revokes unfinished dialogs and jobs. Provider registration for automatic
save, diagnostics, completion, debug, task, or MCP integrations will be added only
with a concrete native lifecycle contract.

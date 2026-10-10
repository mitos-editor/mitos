# plugin-sdk

Rust adapters for Mitos's versioned WebAssembly component world. Public protocol,
capability/error, job, UI, and editor-service types live in `plugin-api`; other
languages generate bindings from `plugin-api/wit/plugin.wit`. The component
interface is `mitos:plugin@0.1.0`, with manifest ABI3.

A Rust plugin is a `cdylib` for `wasm32-unknown-unknown`. Define a
`fn(Request) -> Response` handler and use `export_component!(handle)` or its
`export_plugin!(handle)` alias. Native handler tests remain ordinary Rust tests.
The export macro emits generated canonical component bindings; it does not
export the previous allocation/JSON core ABI.

```rust
use plugin_sdk::{Action, Event, Request, Response, export_plugin};

fn handle(request: Request) -> Response {
    if request.event != Event::Command {
        return Response::default();
    }
    Response {
        actions: vec![Action::Status { message: "Plugin ready".into() }],
        error: None,
    }
}

export_plugin!(handle);
```

Compile the core module and package its embedded interface metadata into a
component before installing it. This world uses Mitos's capability-checked imports and has no WASI imports,
so no WASI adapter is needed.
With `wasm-tools` available, the packaging command is:

```sh
cargo build --target wasm32-unknown-unknown --release
wasm-tools component new target/wasm32-unknown-unknown/release/example.wasm \
    -o example.component.wasm
```

```toml
abi-version = 3
module = "example.component.wasm"
capabilities = ["ui"]

[commands.ready]
doc = "Report plugin readiness"
```

Commands are qualified by the configured package name. `Request.command` carries
its local name, and `Request.args` contains parsed arguments. Configuration and
owned event data are available as JSON values. Init and Shutdown, resynchronization,
state catalogs, and UI/composition responses are targeted controls; ordinary
editor events require manifest subscription and appropriate user grants.

The editor context contains its generation, document metadata, and explicit view
identity/binding/selection revisions. Documents contain `char_count` and
`byte_count`, with no default text. All offsets count Unicode scalar values.
Use `component::read_document(document, version, start, end)` to read one bounded
snapshot region. `Action::Edit` carries the original document version; selection
actions also carry the originating view and its original binding/selection
revisions, even after preceding edits project the resulting selection offsets.

The SDK adapts `Response.actions` into streamed effect resources. It finishes
edit/selection groups, UI prompts/picker rows, builtin compositions, and keymap
bindings before completing the exported handler. Returning `Response.error`
rejects the invocation's staged effects. Finishing a resource does not apply any
editor changes: the handler must succeed and the owning editor validates the
entire response before applying native transactions.

For selection transforms, `transform::selections(&request.editor, convert)` reads
the selected regions and emits one document edit plus revisioned selections.
It preserves multiple selections, their directions, and the primary selection;
all conversions must succeed before any effects are returned. Input and output
are each limited to 1 MiB across at most 128 selections. The
`transform::selections_with_read` variant accepts a reader for native handler tests.
For example, `transform::selections(&request.editor, |text| Ok(text.to_uppercase()))`
converts every selection as one undoable edit.

Bounded host helpers are in `component`:

- `read_document` for a versioned document region.
- `editor_request` for the closed `editor::EditorRequest` schema, including
  document save status, unsaved-buffer lists, bounded syntax/language requests,
  and narrow editor operations.
- `read_file`/`write_file` for relative UTF-8 paths under user-granted roots.
- `read_roots` for up to 16 already granted root indices and their absolute
  canonical/configured paths (64 KiB of total path text). This requires
  `WorkspaceRead` and performs no filesystem I/O or project discovery. Compare
  both path spellings when matching native document paths to a selected root;
  do not guess platform-specific symlink aliases or canonicalize guest paths.
- `storage_read`/`storage_write` for private package storage, when supplied by the host.
- `start_job` with the closed Timer, Search, or Process request types.
  The owned job exposes `poll` and `cancel`; dropping it awaits host cleanup.

`LanguageFormat` and `LanguageCodeActions` query an already attached server, with
source documents limited to 1 MiB; they never start a provider. Code actions
return at most 64 owned choices and 256 total
text edits for the original document/version. Data-only lazy actions may resolve
within the same two-second deadline. Command-bearing, disabled, multi-document,
annotated and file-operation actions are omitted with `truncated = true`.
Queries require `EditorRead`; apply a chosen patch through `Action::Edit`, which
separately requires `EditorEdit` and a still-current document version. Neither
query applies edits or executes an LSP command.

Before running a disk-based project tool, query `DocumentStatus` for the current
version and `UnsavedDocuments` for other modified buffers. The latter defaults
to at most 128 document targets and paths; a truncated reply cannot prove that
the rest of the project is saved. Both queries require `EditorRead`.

For asynchronous workflows, retain the owned `Job` across invocations, record
`Job::id()`, and return from the handler. A targeted `Event::JobReady` carries
that identity in `Request.data["job"]`. Drain `poll` until Pending or Finished on
each readiness event, then return again; several producer chunks may coalesce
into one notification. Drop/cancel finished jobs. Readiness is independent of
the mutation causal chain, and closing the job's original document/view or
unloading its generation cancels the native work.

There is no generic RPC, arbitrary builtin evaluation, shell command string,
ambient filesystem, or WASI. Process requests stream arguments and input through
bounded resources, and remain subject to the user's executable/argument/root
policy. File/storage helpers and UI setters likewise send at most one string per
import to avoid allocation amplification.

Guest execution occurs on dedicated workers. A trap or interrupted Store is
discarded until reload; typed service failures preserve healthy guest state.
All services/effects remain bound to one package and editor generation. See the
[runtime contract](../plugins/RUNTIME.md) for exact memory, message, handle,
action, deadline, and retained-result budgets and their in-process limits.

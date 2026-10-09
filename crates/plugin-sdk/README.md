# Mitos plugin SDK

`plugin-sdk` defines the language-neutral JSON protocol for Mitos plugins and
provides a Rust guest adapter. Plugins run as core WebAssembly modules using
Wasmi, without WASI or host imports. Rust is optional: any language that can
export the memory ABI below can implement the protocol.

The editor integration follows the lifecycle, typable-command, and event-hook
boundaries of [Helix PR #8675](https://github.com/helix-editor/helix/pull/8675),
reviewed at revision `c16fac096a9dd162d46f53bf2411f36251d755f3`. The guest protocol
uses WebAssembly and JSON in place of the PR's Steel bindings.

## Rust guests

Create a `cdylib` with this crate as a dependency and export a request handler:

```rust
use plugin_sdk::{Action, Event, Request, Response, export_plugin};

fn handle(request: Request) -> Response {
    if request.event == Event::Command {
        return Response {
            actions: vec![Action::Status { message: "Hello from Wasm".into() }],
            error: None,
        };
    }
    Response::default()
}

export_plugin!(handle);
```

Build for `wasm32-unknown-unknown`; the macro emits the exports only on Wasm,
so native tests can exercise the same handler. See
[`examples/plugins/uppercase`](../../examples/plugins/uppercase) for a complete
plugin, manifest, and focused Unicode/multiple-selection tests.

## Memory ABI, experimental version 2

A module exports the following names:

| Export | Wasm signature | Contract |
| --- | --- | --- |
| `memory` | linear memory | Contains request and response UTF-8 JSON. |
| `mitos_alloc` | `(i32) -> i32` | Allocate the requested byte length and return its address. |
| `mitos_dealloc` | `(i32, i32) -> ()` | Free an address and its exact allocated byte length once. |
| `mitos_call` | `(i32, i32) -> i64` | Read a request and return the response address in bits 63–32, length in bits 31–0. |

The host allocates the input, writes JSON, invokes `mitos_call`, and frees the
input. The handler only borrows the input: it must not free it. The guest
allocates a separate response; the host copies and frees that allocation. A
guest must keep its linear memory and allocator valid across invocations.
Addresses are unsigned 32-bit bit patterns carried by Wasm `i32`; JSON byte
lengths must be positive and fit in a signed 32-bit integer. A zero packed
response is invalid.

The Rust adapter checks the request's `abi_version`, deserializes it into owned
values, and serializes the response. An invalid JSON request or unsupported ABI
returns a response error. Traps, including panics, are reported by the host.

## JSON protocol

The manifest and request use ABI version `2`. A command request looks like:

```json
{
  "abi_version": 2,
  "event": "command",
  "command": "uppercase",
  "args": [],
  "config": {},
  "editor": {
    "generation": 1,
    "mode": "normal",
    "document": {
      "id": 1,
      "version": 3,
      "path": "/project/example.txt",
      "language": "text",
      "text": "straße"
    },
    "view": {
      "id": 4294967297,
      "document": 1,
      "binding_revision": 0,
      "selection_revision": 3,
      "selections": [{"anchor": 0, "head": 6}],
      "primary": 0
    }
  },
  "data": null
}
```

`command` contains the unqualified name declared in the manifest. `args` are
parsed command arguments, `config` is the plugin's configured JSON value, and
`data` carries event metadata. These four fields can be omitted and default to
`null`, an empty list, `null`, and `null`, respectively. `editor.document` can
be `null`; plugins must handle invocations without a document. Document IDs are
local to the editor session, paths and language identifiers can be `null`, and
the snapshot's version must be supplied with edits. `editor.view` is the originating
split and can be `null` independently of the document. Document-only events and
hidden-document edits do not invent a view. View handles include a slot generation;
`binding_revision` changes on switching documents, including switching away and
back. `selection_revision` advances on explicit selection changes, text mapping,
and undo/redo, even when the text version stays the same. Host generations expire
on reload or shutdown; callbacks and responses from old generations are discarded.
The host validates the originating generation independently of guest data.

Supported event names are `init`, `shutdown`, `command`, `document-opened`,
`document-changed`, `document-saved`, `document-closed`, `selection-changed`,
`mode-changed`, `post-command`, `post-insert-char`, `document-focus-lost`,
`terminal-focus-gained`, `terminal-focus-lost`, `resync-required`, and `state`. Every instance receives lifecycle events;
other hooks are declared in its manifest. Hook data is event-specific, so a
plugin should ignore metadata fields it does not need.

Responses contain an `actions` list and an optional `error` string. Missing
fields default to an empty list and `null`:

```json
{
  "actions": [
    {
      "type": "edit",
      "document": 1,
      "version": 3,
      "edits": [{"start": 0, "end": 6, "text": "STRASSE"}]
    },
    {
      "type": "set-selection",
      "document": 1,
      "version": 3,
      "view": 4294967297,
      "binding_revision": 0,
      "selection_revision": 3,
      "ranges": [{"anchor": 0, "head": 7}],
      "primary": 0
    }
  ],
  "error": null
}
```

Offsets in selections and edits count **Unicode scalar values**, matching
Mitos's document coordinates. They do not count UTF-8 bytes, UTF-16 code units,
or grapheme clusters. A selection covers
`min(anchor, head)..max(anchor, head)`; equal endpoints are an insertion cursor.
An edit replaces `start..end`, excluding `end`. All edits in one `edit` action
refer to the text before that action and must not overlap. Later edit actions
use the projected text after preceding actions. A subsequent `set-selection`
uses coordinates at its position in the action list; further edits map those
selections forward. Every action's `version` remains the original snapshot
version. `set-selection` also supplies the originating view handle and its
original binding/selection revisions; these preconditions are checked against
the current editor before any mutation. Closed documents, changed text, closed
or rebound views, and changed selections produce typed host conflicts. Invalid
ranges or conflicts reject the complete document/selection batch.

Multiple edits to one document compose into a single undo revision. Explicit
selections in multiple splits target those splits. Undo/redo preserves the
originating edit split's selection; other splits' selections map through the
inverse/forward text changes. A text-only operation without an originating view
stores no cursor in its undo history and can edit a hidden document. Readonly
and binary documents reject edits; selection changes can target readonly text.
Both `edit` and `set-selection` must precede any `open` action in a response.

Mutations suppress their originating plugin's own echo; other subscribed plugins
observe them. Every request carries host-owned `data.provenance` with `generation`,
`sequence`, `parent_sequence`, `origin_plugin`, and `depth`, alongside event metadata.
Sequences identify captures, including coalesced snapshots; delivery is not a log
of every intermediate change. Causal notifications stop after depth 8.

Document/selection changes coalesce by document and originating view. Queues retain
at most 32 data events, 32 control events, and 8 MiB of owned data. Overflow,
oversized snapshots, and causal limits produce mandatory `resync-required` events
with dropped sequence ranges, counts, and reasons. A final shutdown gap adds
`closing: true`; state queries are then rejected. Shutdown drains accepted save,
close, and post-command hooks before invalidating the generation. Saved events
carry the actual written snapshot and `path`, `saved_revision`, `saved_version`,
`current_version`, and `snapshot_available`, including write-and-quit.

Return `{"type":"request-state","query":{}}` to receive a targeted `state` event,
without a manifest subscription. Its data contains `StateCatalog`: document/view
metadata, exclusive `next_document`/`next_view` cursors, and an optional error.
Query `after_document`, `after_view`, and `limit` for additional pages (at most 64
entries per catalog). Optional `document` or `view` handles retrieve current
snapshots in `editor.document`/`editor.view`; closed targets and oversized snapshots
return explicit query errors. Query completion goes only to the requesting plugin.

Available actions are `edit`, `set-selection`, `status` (a `message`), `error`
(a `message`), `open` (a `path`), and `request-state` (a `query`). Response errors report a failed invocation;
plugins should return no actions when reporting a failure.

The initial API intentionally has a small synchronous surface. It does not
provide arbitrary editor-command execution, LSP calls, Tree-sitter handles,
custom UI, asynchronous work, or direct filesystem/network access. Modules
execute with fixed host memory, instruction, and response limits. Document
snapshots are limited to 2 MiB of text; JSON requests and responses are limited
to 4 MiB. Plugins can read the provided document snapshot and request only the
actions above.

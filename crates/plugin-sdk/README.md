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

## Memory ABI, version 1

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

The manifest and request use ABI version `1`. A command request looks like:

```json
{
  "abi_version": 1,
  "event": "command",
  "command": "uppercase",
  "args": [],
  "config": {},
  "editor": {
    "mode": "normal",
    "document": {
      "id": 1,
      "version": 3,
      "path": "/project/example.txt",
      "language": "text",
      "text": "straße",
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
the snapshot's version must be supplied with edits.

Supported event names are `init`, `shutdown`, `command`, `document-opened`,
`document-changed`, `document-saved`, `document-closed`, `selection-changed`,
`mode-changed`, and `post-command`. Every instance receives lifecycle events;
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
refer to the original snapshot and must not overlap. A subsequent
`set-selection` uses coordinates after those edits, while its `version` remains
the original snapshot's version, and `primary` indexes its `ranges` list. Stale
document versions and invalid bounds are rejected by the editor. Both `edit`
and `set-selection` must precede any `open` action in a response, because opening
a path can replace the view holding their document.

Editing and setting selections currently require a document with a visible
view. Editor mutations requested by plugins do not invoke hooks on any plugin.
Pending document-change and selection-change hooks are coalesced per document,
and the pending event queue retains at most 32 snapshots; hooks do not guarantee
notification of every intermediate change.

Available actions are `edit`, `set-selection`, `status` (a `message`), `error`
(a `message`), and `open` (a `path`). Response errors report a failed invocation;
plugins should return no actions when reporting a failure.

The initial API intentionally has a small synchronous surface. It does not
provide arbitrary editor-command execution, LSP calls, Tree-sitter handles,
custom UI, asynchronous work, or direct filesystem/network access. Modules
execute with fixed host memory, instruction, and response limits. Document
snapshots are limited to 2 MiB of text; JSON requests and responses are limited
to 4 MiB. Plugins can read the provided document snapshot and request only the
actions above.

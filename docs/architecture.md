
| Crate           | Description                                                      |
| -----------     | -----------                                                      |
| stdx      | Extensions to the standard library (similar to [`rust-analyzer`'s](https://github.com/rust-lang/rust-analyzer/blob/ea413f67a8f730b4211c09e103f8207c62e7dbc3/crates/stdx/Cargo.toml#L5)) |
| core      | Core editing primitives, functional.                             |
| lsp       | Language server client                                           |
| lsp-types | Language Server Protocol type definitions                        |
| dap       | Debug Adapter Protocol (DAP) client                              |
| event     | Primitives for defining and handling events within the editor    |
| loader    | Functions for building, fetching, and loading external resources |
| spelling  | Dictionary operations, scanning, suggestions, and word persistence |
| view      | UI abstractions for use in backends, imperative shell.           |
| term      | Terminal UI                                                      |
| tui       | Ratatui integration and Mitos-specific terminal rendering         |


This document contains a high-level overview of Mitos internals.

> NOTE: Use `cargo doc --open` for API documentation as well as dependency
> documentation.

## Core

The core contains basic building blocks used to construct the editor. It is
heavily based on [CodeMirror 6](https://codemirror.net/6/docs/). The primitives
are functional: most operations won't modify data in place but instead return
a new copy.

The main data structure used for representing buffers is a `Rope`. We re-export
the excellent [ropey](https://github.com/cessen/ropey) library. Ropes are cheap
to clone, and allow us to easily make snapshots of a text state.

Multiple selections are a core editing primitive. Document selections are
represented by a `Selection`. Each `Range` in the selection consists of a moving
`head` and an immovable `anchor`. A single cursor in the editor is simply
a selection with a single range, with the head and the anchor in the same
position.

Ropes are modified by constructing an OT-like `Transaction`. It represents
a single coherent change to the document and can be applied to the rope.
A transaction can be inverted to produce an undo. Selections and marks can be
mapped over a transaction to translate to a position in the new text state after
applying the transaction.

> NOTE: `Transaction::change`/`Transaction::change_by_selection` is the main
> interface used to generate text edits.

`Syntax` is the interface used to interact with tree-sitter ASTs for syntax
highlighting and other features.

## View

The `view` layer was supposed to be a frontend-agnostic imperative library that
would build on top of `core` to provide the common editor logic. Currently it's
tied to the terminal UI.

`view::config` defines editor configuration types, defaults, and serialization.
`term::config` loads and merges configuration files, while `view::editor` owns
editor state and applies configuration updates. Configuration types remain
re-exported from `view::editor` for compatibility; new code should use
`view::config`.

Terminal capability overrides live in `term::config::TerminalConfig`. The
terminal application composes them with `view::config::Config` through an
`EditorSettings` representation that preserves the `[editor]` configuration
table and the `:get`, `:set`, and `:toggle` commands. Frontend command contexts
receive a configuration snapshot and an update sender; the application applies
updates and reconfigures its backend. Shared editor events continue to carry
only shared settings.

`view::editor` implements configuration application in `editor/config.rs`.
`Editor::load_language_config` updates workspace trust before loading language
resources; `apply_language_config` installs the loader and selected theme before
refreshing document language settings, `.editorconfig`, and diagnostics.
`Editor::refresh_config` updates derived editor state, dispatches change hooks,
and adjusts every view after those hooks run. The frontend installs its settings
snapshot before calling this final refresh.

`term::Application` loads the combined application configuration, selects a theme
using terminal capabilities, reconfigures the backend, and publishes the snapshot
used by editor settings and keybindings. Reload preserves existing failure
boundaries: an application-config error changes no resources; a language-load
error retains the trust update but leaves the active loader intact. Theme errors
still allow document refresh. Configuration-event reloads still run change hooks
after failure, using the currently installed settings.

A `Document` ties together the `Rope`, `Selection`(s), `Syntax`, document
`History`, language server (etc.) into a comprehensive representation of an open
file.

A `View` represents an open split in the UI. It holds the currently open
document ID and other related state. Views encapsulate the gutter, status line,
diagnostics, and the inner area where the code is displayed.

> NOTE: Multiple views are able to display the same document, so the document
> contains selections for each view. To retrieve, `document.selection()` takes
> a `ViewId`.

`Info` is the autoinfo box that shows hints when awaiting another key with bindings
like `g` and `m`. It is attached to the viewport as a whole.

`Surface` is like a buffer to which widgets draw themselves to, and the
surface is then rendered on the screen on each cycle.

`Rect`s are areas (simply an x and y coordinate with the origin at the
screen top left and then a height and width) which are part of a
`Surface`. They can be used to limit the area to which a `Component` can
render. For example if we wrap a `Markdown` component in a `Popup`
(think the documentation popup with space+k), Markdown's render method
will get a Rect that is the exact size of the popup.

Widgets are called `Component`s internally, and you can see most of them
in `crates/term/src/ui`. Some components like `Popup` and `Overlay` can take
other components as children.

`Layer`s are how multiple components are displayed, and is simply a
`Vec<Component>`. Layers are managed by the `Compositor`. On each top
level render call, the compositor renders each component in the order
they were pushed into the stack. This makes multiple components "layer"
on top of one another. Hence we get a file picker displayed over the
editor, etc.

The `Editor` holds the global state: all the open documents, a tree
representation of all the view splits, the configuration, and a registry of 
language servers. To open or close files, interact with the editor.

`loader::syntax::Resources` owns the ordered runtime paths used to load native
grammars and query sources, including inheritance. Reads stay lazy and resolve
each file independently, so partial overrides preserve lower-priority resources.
Missing grammars remain optional; unreadable query files remain empty. Existing
health-check entry points retain raw-file I/O errors and process-default paths.

`core::syntax::queries` compiles supplied grammars and source text without file
I/O. `core::syntax::Loader` holds an explicit resource source and per-language
lazy compilation caches, including failed compilations. A new loader starts fresh
caches. `view::Editor::load_language_config` refreshes trust and retains the active
loader's resource paths; `apply_language_config` installs the replacement before
refreshing documents. `xtask query-check` uses `Loader::validate_queries`, sharing
the editor's loading and compilation paths while returning errors to the caller.

`view::handlers::syntax` schedules background syntax initialization and publishes
results through an explicit `view::callbacks::EditorCallbackSender`. The editor
attaches its syntax handler to each document before initialization, so initial
requests and retries return to the same editor. Snapshot computation, worker
limits, cancellation, and version/language validation live alongside the document
in `document/syntax_initialization.rs`. Successful publication refreshes spelling.

`spelling` owns dictionary loading from explicit file paths, compiled token
filters, text/syntax scanning, suggestions, personal vocabulary snapshots, and
persistent ignored-word lists. Its API accepts text, syntax, settings,
dictionaries, scan budgets, and cancellation checks; it returns character-indexed
diagnostics without an editor or async runtime. `core` retains the serialized
spelling configuration and language identifiers used by language configuration.
The dependency direction is `view` → `spelling` → `core`.

`view::handlers::spelling` selects dictionary resources, combines effective
settings, owns per-editor dictionary/ignore state, and coordinates incremental
and background checks. Document events and callbacks stay with the owning editor;
cancellation, version/language validation, diagnostic publication, and applying
corrections remain in `view`. Spelling commands and suggestion menus live in `term`.

`view::handlers::{document_symbols, document_highlight, document_colors,
document_links}` own the corresponding LSP requests, debounce queues, event
hooks, and publication. Each document receives its editor's feature handles;
hook registration happens once, and document events use those handles rather
than a captured application queue. Request snapshots validate cancellation,
document version and URI, and server attachment when queued results are applied.
Highlights also validate the view's document and selection. Cached data and
request controllers follow the document lifetime; rendering and interactive
commands remain in the frontend.

`Editor::handle_language_server_initialized`, `handle_publish_diagnostics`, and
`handle_language_server_exit` in `view::handlers::lsp` own editor-side lifecycle
and push-diagnostic handling. Initialization queues configuration before hooks
open documents and start feature requests. Push diagnostics validate the URI and
server readiness, then use the shared version, provider, and persistence rules.
Exit clears the server's diagnostics from open documents and the workspace cache,
runs feature cleanup while the server is still registered, then removes it.
`term::Application` decodes incoming notifications and calls these operations;
exit status text, progress displays, and interactive prompts stay in the frontend.

`view::handlers::lsp::workspace` owns server-requested workspace edit validation,
application results, and dynamic file-watcher registration. It uses the server's
negotiated position encoding, preserves partial edit failures, and adds relative
watch roots before registering interests. `lsp::Client::configuration` resolves
ordered configuration sections; workspace folders remain on the client.
`term::Application` decodes requests, calls these typed operations, and serializes
and sends replies. Diagnostic refresh uses the existing shared pull-diagnostic
entry point. Unsupported capability registrations retain their compatibility
acknowledgement; progress and interactive requests stay with the frontend.

`loader::workspace_trust` owns trust policy, persisted grants/exclusions, and
workspace configuration hashing. `view::handlers::workspace_trust` owns each
editor's prompt history, pending requests, decisions, and service effects.
Requests arrive through `EditorEvent::WorkspaceTrust`; delivery and resolution
validate the owning editor, live workspace, current policy, and configuration
snapshot. Explicit decisions invalidate pending prompts and request a configuration
reload. Modal acceptance launches missing language servers; explicit trust commands
restart the current document's selected servers through
`Editor::restart_language_servers`, also used by `:lsp-restart`. Revoking or excluding
trust leaves running servers alone. `term` owns modal text, labels, and command adapters.

`view::handlers::auto_reload` owns external-change detection, polling, reload
decisions, split synchronization, VCS refresh, and ignore-filter refresh. Polls
use an editor-owned callback destination; weak ownership prevents closed or
replaced handlers from retaining or mutating an editor. Focus events call the
shared check for unwatched files.

`view::file_watcher` owns native watcher configuration, lifecycle, workspace and
LSP roots, coverage checks, and VCS metadata polling. Its `filter` module owns
ignore rules, hidden-path filtering, and depth limits. Native callbacks target
only their owning editor. One forwarding task preserves batch order and waits
for callback queue capacity without blocking native watcher threads. A watcher
generation rejects queued events after reconfiguration, disabling, or replacement;
those transitions also cancel pending delivery. No global filesystem hook is needed.

`Editor::handle_file_events` forwards each batch to that editor's LSP clients
before handling automatic reload, VCS, and ignore updates. `lsp::file_event`
accepts paths and LSP change kinds, retaining registration matching, batching,
and deduplication without depending on `view` or a native watcher. Temporary-file
events are omitted at the editor boundary. `stdx::path::canonicalize_existing`
resolves symlink ancestors for both existing and missing paths. `core` has no
direct watcher, ignore-filter, event, or Tokio dependency.

Modified buffers produce a typed `ReloadRequest`, delivered to frontends through
`EditorEvent::ReloadConfirmation`. `term` displays the prompt and returns a
`ReloadDecision`. Shared code validates the owning handler, document/path,
version, saved timestamp, current file timestamp, and settings before applying
it. Stale requests cannot overwrite newer state or suppress a newer confirmation;
a later check can request a fresh decision. Reload uses existing transactions,
undo history, selection mapping, and per-document Git trust checks.

`view::handlers::diagnostics::pull` owns pull-diagnostic requests, document and
inter-file debounce queues, server-requested retries, and report publication.
Result IDs and cancellation controllers live in feature state attached to each
document and are scoped per server. A provider refresh affects only that provider;
queued reports and retries validate their document snapshot and server attachment.
The terminal application calls shared refresh entry points for server progress and
diagnostic refresh requests. Diagnostic visibility during mode changes, panels,
and pickers stay in `term`.

`view::handlers::code_action_hint` owns debounced code-action availability checks
for each document/view. `view::action::code_actions_for_range` builds LSP requests
for hints, interactive actions, and actions on save. Documents retain hint state,
cancellation controllers, and their editor's scheduling handle. Selection,
document, diagnostic, and configuration changes invalidate pending results;
publication also checks the view, document snapshot, and server attachment.
Spelling findings provide hints even without an LSP server. Statusline and gutter
indicators, the picker, and save-job sequencing remain in `term`.

`view::handlers::signature_help` owns manual and automatic triggers, debounce
timing, server selection, cancellation, and response validation. Documents hold
weak scheduling handles to their editor's coordinator. Requests retain document,
view, selection, and server snapshots; newer requests and lifecycle changes
invalidate both queued callbacks and results awaiting presentation. Typed updates
arrive through `EditorEvent::SignatureHelp` and are resolved against the receiving
editor before display. `term` forwards mode and insertion events and owns popup
formatting, placement, signature navigation, and completion-menu overlap.

`view::handlers::completion` owns completion items, LSP/path/word providers,
request debouncing, batching, incomplete-list refreshes, resolution, and
cancellation. Editor-owned callbacks publish typed `EditorEvent::Completion`
updates that validate their session again before presentation. Sessions retain
provider savepoints across normal typing and ghost previews; cancellation,
replacement, focus changes, and stale targets prevent old results from being
applied. `term` forwards input and owns menu filtering, navigation, documentation,
and preview/acceptance UI, including insert-repeat history.

`loader::theme::Resources` owns theme file discovery, embedded default sources,
inheritance resolution, and TOML merging. It searches explicitly supplied roots
in priority order. Same-name inheritance can continue in a lower-priority root;
visited paths detect cycles. Style entries replace parent entries, while palette
entries retain the existing deeper merge. Sources remain lazy and uncached.

`view::theme::Loader` receives those resources and interprets their merged data
as editor styles, palettes, and highlight scopes, preserving warnings and cached
built-in `Theme` values. `term` selects the initial roots, adaptive light/dark
themes, and terminal capability fallback. Theme completion enumerates the active
editor's resources, and `xtask theme-check` uses the same discovery and loading
path with bundled-runtime roots. Configuration reloads continue to use the
editor's selected theme resources.

`view::icons` owns shared icon data, theme lookup, glyph padding, and width
calculations used by editor gutters and frontends. `term::ui::icon_span` converts
that data into Ratatui text for pickers, completion, the bufferline, and statusline.
`view` has no direct Ratatui dependency; shared geometry and style types still
come through `ui-core`.

Clipboard settings and execution have separate owners. `view::clipboard::ClipboardProvider`
is the serializable provider choice; `Clipboard` holds live configuration access
and an application-supplied `ClipboardBackend`. Registers own saved selections,
clipboard/primary selection mapping, and fallback for providers that cannot read.
Each operation takes one settings snapshot, so configuration updates affect the
next operation without replacing saved registers or the backend.

The shared `NativeClipboard` backend runs native/custom commands and platform APIs.
`Editor::new` installs that backend by default. `term::clipboard` owns terminal
provider selection and OSC 52 output; `Application` installs its backend before
using the editor. Frontends can replace clipboard access through
`Registers::set_clipboard_backend`, including with in-memory access for tests.
Shared defaults never select terminal output; the terminal configuration layer
preserves the existing detection order and fallback. The `[editor.clipboard-provider]`
schema, provider names, and command behavior remain unchanged. `view` has no direct
terminal-output dependency or terminal feature flag for clipboard access.

`view::handlers::Handlers::new` constructs shared editor services from editor
configuration and an explicit callback destination. It registers editor events
and feature hooks once per event registry, including LSP notifications, snippet
range tracking, and filesystem configuration updates. A frontend can initialize
these services without terminal setup. `term::handlers::register_hooks` installs
terminal input and presentation hooks separately.

Word-index hooks route open/configuration events through the supplied editor and
edit/close events through a weak document-bound sender. Each editor owns its
index, debounce queue, and worker lifetime. Inserts, edits, closes, and resets
share an ordered queue; resetting discards pending edits before rebuilding from
current document settings, including language overrides. Repeated initialization
does not duplicate LSP notifications, snippet transformations, or index entries.

The terminal application supplies a sender backed by its existing bounded job
queue, which applies editor-only callbacks in the event loop. Async sends wait
for capacity; synchronous sends retain the queue's bounded-wait policy. Syntax
initialization, spelling, document features, pull diagnostics, code-action hints,
automatic reload, autosave, signature help, and completion use explicit destinations.
Other background features still use the existing terminal handlers and dispatch
paths.

`view::editing::replace_selections` replaces a document's selections through one
transaction and commits pending edits to history. It takes a `Document`, a `View`,
and replacement text, preserving selection direction and the primary selection.
Text-only newline normalization lives in `core::line_ending`, shared with paste
and register replacement. The terminal adapter owns prompts, repeat recording,
mode transitions, and scrolling; prompted replacement and repeat use the same
editor operation.

`view::save` owns save options, whitespace/newline normalization, history
checkpoints, modified-document selection, and save-all error policy. `prepare`
returns a request for one document; `save_all` prepares and submits each eligible
document through a caller-supplied callback. Preparation happens immediately
before submission, so an immediate error leaves later documents unprepared.
`auto_save` uses this policy to enqueue writes without formatting or code actions,
skipping scratch buffers. `Editor::save` owns the underlying write queue.

## LSP

A language server protocol client.

## Term

The terminal frontend.

The `main` function sets up a new `Application` that runs the event loop.

`commands.rs` wires feature modules and preserves public entry points. Command
implementations import their dependencies explicitly; they do not inherit a shared
set of imports from the facade.

| Module in `commands/` | Responsibility |
| --- | --- |
| `context.rs` | Command context, next-key callbacks, and job/compositor adapters |
| `mappable.rs` | Command representation, parsing, deserialization, and execution |
| `catalog.rs` | Static and typable declarations, names, aliases, signatures, and handler paths |
| `command_line.rs` | Prompt phases, completion, help, custom-command expansion, and dispatch |
| `editing.rs`, `insert.rs`, `selection.rs`, `movement.rs` | Text transformations, insertion, selections, and cursor movement |
| `mode.rs`, `history.rs`, `formatting.rs` | Mode adaptation, edit-history commands, and formatting jobs |
| `navigation.rs` | Buffer/location jumps, jump history and its picker, and word-label jumps |
| `files.rs`, `buffers.rs` | File links, file pickers/explorers, open/save/reload commands, document properties, and buffer lifecycle/picker commands |
| `windows.rs` | Split creation, focus, arrangement, and window closing |
| `diagnostics.rs` | Diagnostic navigation, pickers across all providers, and diagnostic yanking |
| `spelling.rs` | Spelling navigation, finding ranges, and spelling-language commands |
| `quicklist.rs` | Quicklist traversal and its picker |
| `vcs.rs` | Change navigation, changed-file pickers, and change-reset commands |
| `search.rs` | Buffer search, search registers, workspace search, and search result selection |
| `syntax.rs`, `textobjects.rs` | Syntax traversal/inspection/symbol queries and text-object selection |
| `lsp.rs`, `dap.rs`, `shell.rs` | Language-server, debugger, and shell command adapters |
| `symbols.rs` | Selection between LSP and syntax symbol providers |
| `completion.rs`, `snippets.rs`, `macros.rs` | Completion triggers, snippet navigation, and macro recording/replay |
| `config.rs`, `workspace.rs`, `application.rs` | Configuration requests, directory/trust commands, and application lifecycle commands |
| `palette.rs` | Command palette and reopening the last picker |
| `picker.rs` | Shared path styling for feature-owned pickers |

Features keep typable handlers in the same file as their static commands and
helpers. Mixed features use an inline `mod typed` to distinguish invocation
signatures and avoid name collisions. Configuration and workspace handlers live
directly in their feature modules. The catalog points at the owning feature and
namespace.
`commands/typed.rs` is a compatibility facade for metadata, completion, and shared
save exports; it contains no handlers.

Helpers stay with their feature and use narrow visibility for sibling callers.
For example, navigation owns jump recording, formatting owns its result callback,
and spelling and VCS expose range helpers to text-object selection. Picker
construction lives with the feature that supplies its content.

These modules adapt counts, registers, prompt phases, selections, and terminal
jobs. Document state, diagnostic storage, quicklists, and editor operations remain
in `view`; text algorithms remain in `core`; protocol and repository operations
remain in their integration crates. File commands use `view::save` for shared
preparation and batch policy. Their terminal adapter schedules code actions,
formatting, and the final write in order, retaining document/version checks and
exit waiting. `view::handlers::auto_save` owns debouncing, insert-mode deferral,
focus-loss policy, and save submission through `view::save::auto_save`. Documents
schedule changes through weak handles to their editor's coordinator; callbacks
return through its explicit sender and reject replaced handlers, newer edits, and
disabled autosave. `term` forwards mode changes and focus loss and renders editor
errors. The command catalog remains static.

`keymap.rs` links commands to key combinations.


## TUI / Term

TODO: document Component and rendering related stuff

## Event

The `event` crate defines primitives for defining and acting on events
within the editor. "Events" cover things like opening, changing and closing of
documents, starting and stopping of language servers and more.

`event` has tools for defining events and registering _hooks_ which run
any time an event is emitted. `event` also provides `AsyncHook` - a tool
for running cancellable tasks which run after events with _debouncing_.

See the `AsyncHook` type for more information. Events can be created within the
`events!` macro. Synchronous hooks can be created with `register_hook!`. And
editor-wide events can be sent to hooks with `event::dispatch`.

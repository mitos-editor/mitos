# Architecture

Mitos separates text algorithms, shared editor behavior, and terminal interaction.
Features live with the state they change; frontends adapt input and present results.

| Crate | Responsibility |
| --- | --- |
| `core` | Ropes, selections, transactions, and text editing algorithms |
| `text-layout` | Display coordinates, wrapping, annotations, and visual movement |
| `buffer` | Text state, versions, pending undo groups, and history ownership |
| `language` | Language definitions, syntax trees, queries, resource loading, and structural editing |
| `editor-config` | Editor settings, defaults, custom command definitions, and serialization |
| `language-config` | Language definitions and server feature configuration |
| `view` | Documents, views, editor operations, configuration application, and feature coordination |
| `theme` | Theme interpretation, palette resolution, and style lookup |
| `term` | Application composition, configuration loading, terminal input, commands, and interactive UI |
| `ui-core` | Shared geometry, input, typed keymap lookup, colors, and styles |
| `tui` | Terminal backends, capability handling, and Ratatui integration |
| `loader` | Resource paths and grammar/query/theme sources |
| `grammar-build` | Grammar source fetching and native compilation |
| `clipboard` | Clipboard provider settings and native backends |
| `search` | Workspace traversal and text matches over files or unsaved snapshots |
| `text-io` | Streaming text decoding, encoding detection, BOM handling, and encoded writes |
| `workspace` | Workspace discovery, trust policy and persistence with an explicit store directory |
| `spelling` | Dictionary operations, scanning, suggestions, and word persistence |
| `lsp`, `dap`, `vcs` | Language-server, debugger, and repository integration |
| `lsp-types`, `dap-types` | Protocol data types |
| `command-line`, `parsec`, `snippets` | Command argument parsing, parser combinators, and snippet parsing/state |
| `event` | Event registration, cancellation, and debounced task primitives |
| `stdx` | Shared standard-library utilities |
| `xtask` | Repository maintenance, validation, and generated documentation |

`term` composes `view` with `tui`; shared editor behavior does not depend on either
terminal crate. `view` coordinates feature and protocol crates. For example,
`view` → `spelling` → `core` keeps dictionary algorithms independent of editor
state. `ui-core` supplies UI value types without owning editor services.
Component keymap parsing and prefix lookup live in `ui-core`; action definitions,
defaults, configuration composition, and execution belong to `term`. Buffer picker
item actions call editor lifecycle operations from `term::commands::buffers`.
Editor mode keymaps and their menu and binding labels also belong to `term`,
where static commands, typable commands, and macros are resolved and executed.
They reuse input types from `ui-core`; `command-line` only parses command arguments.

Picker matching, input, and composition live in `term::ui::picker`. Its private
`preview` module owns preview caches, loading, cancellation, asynchronous
publication, and rendering. Dynamic query scheduling remains in `handlers`. Query callbacks validate
the originating picker's generation and query before restarting its provider.
Prompt cleanup captures the prompt instances present at the focus change; queued
cleanup cannot remove a later prompt in the same application.

Use `cargo doc --open` for API documentation. Crate READMEs summarize ownership;
this guide explains how those owners cooperate.

## Where new code belongs

| Change | Home |
| --- | --- |
| Text transformation independent of open buffers | `core` |
| Syntax analysis, query compilation, or structural editing | `language` |
| Operation on documents, selections, history, or editor services | `view` |
| Document feature cache and request lifetime | With its coordinator in `view/src/features/<feature>.rs` or its feature directory |
| Feature scheduling, event hooks, and result publication | The same `view/src/features` module as its state |
| Resource discovery or source inheritance | `loader` |
| Grammar fetching or compiler invocation | `grammar-build` |
| Protocol request/response transport | Its protocol crate; editor effects belong in `view` |
| Keybinding, prompt, picker, popup, or terminal rendering | `term` |
| Backend output or terminal capabilities | `tui` |

Keep helpers beside their callers and share them when concrete consumers need
the same policy. Introduce a feature crate when it has useful explicit inputs and
outputs independent of editor coordination, as `spelling` does. Existing command
catalogs and feature wiring are static; this organization does not require a plugin API.

The `workspace` crate depends only on `stdx`; applications supply its trust-store
directory. Configuration migration
passes explicit file changes to trust policy without a dependency back on `loader`.

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

`core::selection::operations` owns whitespace trimming, copying at document cell
columns, whole-line selection, and line-bound transformations. These functions
return a selection without accessing a document or view. Terminal commands supply
text and options, then apply results through `Document::set_selection` so shared
selection events continue to run.

Ropes are modified by constructing an OT-like `Transaction`. It represents
a single coherent change to the document and can be applied to the rope.
A transaction can be inverted to produce an undo. Selections and marks can be
mapped over a transaction to translate to a position in the new text state after
applying the transaction.

> NOTE: `Transaction::change`/`Transaction::change_by_selection` is the main
> interface used to generate text edits.

Text selections, indentation normalization, comments, and visual annotations use
text-only APIs. Parser-node conversion and structural editing live in `language`.
Highlight IDs and overlay traversal live in `core::highlight`; the syntax
highlighter adapts parser output to those shared IDs without allocation.

## Text layout

`text-layout::DisplayPosition` identifies formatted rows and terminal-cell columns,
including soft wraps and virtual lines. Formatters, visual coordinate mapping,
annotations, cursor caches, and text rendering use this type. `core::Position`
continues to describe document coordinates. Call the layout mapping APIs to cross
between character offsets and display positions; the types do not implicitly convert.
View methods explicitly translate layout positions into viewport coordinates.

## Language

`language::Syntax` owns incremental syntax trees, including language injections.
`language::Loader` resolves language metadata and lazily compiles grammar queries
from explicit resources. Structural matching, movement, text objects, and
query-based indentation live beside syntax analysis.

`language` depends on `core`; `core` has no grammar, runtime-loader, workspace,
protocol, editor, or frontend dependency. Structural editing lives in `language`;
plain word/paragraph motion, bracket scanning, and indentation normalization stay
in `core`. Configuration loading uses `language::loader`, schema definitions use
`language-config`, and query compilation uses `language::queries`.

## View

`view` owns the shared editor model and operations. It can run without a terminal
compositor; frontends supply callback delivery and presentation. Presentation-related
model data, such as view geometry, theme styles, and gutter settings, remains shared.

### Mode transitions

`Editor::set_mode` in `editor/mode.rs` owns mode changes, feature cancellation,
insert-mode triggers, diagnostic activity, and deferred autosave. Normal mode also
restores indentation and the append cursor. `ModeDidChange` observers run after
those required effects. Terminal hooks retain menu presentation and edit replay;
headless callers use the same transition operation.

### Configuration application

`editor-config` defines editor configuration types, defaults, and serialization.
`term::config` loads and merges configuration files, while `view::editor` owns
editor state and applies configuration updates. Import shared settings from
`editor-config`, their owning module.

Terminal capabilities, keyboard protocol, and external launcher settings live
in `term::config::TerminalConfig` under `[terminal]`. The terminal application
composes them with `editor_config::Config` through an `EditorSettings` representation
that exposes editor-relative paths and `terminal.*` paths to the `:get`, `:set`,
and `:toggle` commands. Frontend command contexts receive a configuration snapshot and an update sender; the application applies
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

### Documents and views

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

The `Editor` holds the global state: all the open documents, a tree
representation of all the view splits, the configuration, and a registry of 
language servers. To open or close files, interact with the editor.
Its private `editor/documents.rs` module owns document registration, loading,
view binding, and closure. `editor/filesystem.rs` owns path operations and their
watcher and language-server effects. Both implement the existing `Editor` API;
callers do not coordinate these steps themselves.

`Editor::open_scratch` loads a fresh text copy before attaching it to editor
services. The copy retains its source language and encoding without acquiring
a file identity. It cannot overwrite its source through an unnamed save.
Low-level document path mutation is private to `view`; registered documents
change paths through `Editor::set_doc_path`.

### Debugger state

`view::debugger::DebuggerState` owns debug-adapter sessions and source breakpoints.
`Editor::debugger` exposes read access; mutations use session and breakpoint
operations through `Editor::debugger_mut`. Terminal commands collect input and
present debugger output. Breakpoint toggles, property changes, and adapter
synchronization belong to the shared owner, including before a session starts.

Breakpoint prompts retain an editor-owned `BreakpointId`, independent of the adapter's
optional session ID and reported line. Removing a breakpoint invalidates that target;
verification and relocation preserve it. New property values are checked against adapter
capabilities before storage. Rejected properties leave the previous value intact;
clearing an unsupported property is always
permitted, even when other settings still prevent adapter synchronization.

Debugger terminal launching is a frontend operation injected into `DebuggerState`
before adapters start. DAP advertises terminal support only when that operation is
installed. Shared reverse-request handling returns success or an error to the
adapter on every path. Terminal startup runs outside the editor mutation loop;
its completion callback checks the originating adapter ID before replying.
`term::terminal_launcher` owns launcher discovery and the
private helper that applies cwd/environment inside the destination terminal,
including multiplexer sessions. A private temporary handoff carries process inputs;
only its path appears on the launcher command line. The helper returns the actual
debuggee process ID, then waits for its exit.

### Resources and feature coordination

`grammar-build` fetches grammar sources and compiles native libraries from explicit
grammar definitions, a runtime output directory, and an optional compilation target.
CLI and build-script callers resolve configuration and persisted workspace trust
through `loader` before invoking it. Runtime consumers do not depend on compiler
tooling; release CI uses the small `grammar-fetch` executable.

`loader::syntax::Resources` owns the ordered runtime paths used to load native
grammars and query sources, including inheritance. Reads stay lazy and resolve
each file independently, so partial overrides preserve lower-priority resources.
Missing grammars remain optional; unreadable query files remain empty. Existing
health-check entry points retain raw-file I/O errors and process-default paths.

`language::queries` compiles supplied grammars and source text without file
I/O. `language::Loader` holds an explicit resource source and per-language
lazy compilation caches, including failed compilations. A new loader starts fresh
caches. `view::Editor::load_language_config` refreshes trust and retains the active
loader's resource paths; `apply_language_config` installs the replacement before
refreshing documents. `xtask query-check` uses `Loader::validate_queries`, sharing
the editor's loading and compilation paths while returning errors to the caller.

`view::features::syntax` schedules background syntax initialization and publishes
results through an explicit `view::callbacks::EditorCallbackSender`. The editor
attaches its syntax handler to each document before initialization, so initial
requests and retries return to the same editor. Snapshot computation, worker
limits, cancellation, and version/language validation live alongside the document
in `document/syntax_initialization.rs`. Successful publication refreshes spelling.

`vcs::FileBlame` owns immutable committed-line ranges and commit metadata.
`view::features::blame` owns caching, cancellation, request trust decisions,
scheduling for visible documents, and publication to their owning editor. Cache reuse and queued publication validate effective workspace Git trust.
Line lookup uses only a diff of the current document snapshot. Commands and inline
decorations belong to `term`, where diagnostics take precedence over blame text.

`spelling` owns dictionary loading from explicit file paths, compiled token
filters, text/syntax scanning, suggestions, personal vocabulary snapshots, and
persistent ignored-word lists. Its API accepts text, syntax, settings,
dictionaries, scan budgets, and cancellation checks; it returns character-indexed
diagnostics without an editor or async runtime. `language::config` owns serialized spelling settings; `core` retains dictionary
identifiers. Scanning depends on `language` for syntax and on `core` for text and
diagnostics. Both remain independent of editor coordination.

`view::features::spelling` selects dictionary resources, combines effective
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

`features::request` defines document, view-bound, and selection-bound snapshots.
View and selection snapshots return validated references for feature policy checks;
they also provide the target IDs and cancellation handle used by publication.
Focus, mode, provider choice, handler ownership, and viewport coverage stay in the
feature. Ordinary completion retains its savepoint-based session, which remains
valid across typing and temporary previews.

Document colors, links, highlights, code-action hints, and symbols/breadcrumbs
have state modules under `view/src/features/`. Each groups its cache, request
controllers, and attached scheduling handle. Pull diagnostics keeps its handle
with its diagnostic state. Removing a view cancels its per-view work; dropping a
document drops its request controllers. Cache-only clearing preserves replacement
requests for colors, links, highlights, and symbols. Clearing code-action hints
also invalidates their requests. Each feature makes that distinction explicit.

Colors, links, and pull diagnostics share the internal `features::debounce` helper.
Each keeps its own queue and request policy; inter-file diagnostic refresh retains
its separate timing. `DocumentRequest` validates cancellation, document version,
URI, and server attachment before queued results mutate editor state.

Language-server attachment lives in `editor/language_servers.rs`. Transport
readiness and editor initialization are distinct: the editor queues configuration,
opens each pending document with its current snapshot once, and then notifies
feature observers. Documents expose providers only after their protocol document
has opened; edits, saves, renames, and closure respect that state.

`Editor::handle_language_server_initialized`, `handle_publish_diagnostics`, and
`handle_language_server_exit` in `view::handlers::lsp` own editor-side lifecycle
and push-diagnostic handling. Initialization queues configuration and opens protocol documents before
notifying feature observers. Push diagnostics validate the URI and
server readiness, then use the shared version, provider, and persistence rules.
Exit clears the server's diagnostics from open documents and the workspace cache,
runs feature cleanup while the server is still registered, then removes it.
`Editor::stop_language_servers` and `restart_language_servers` in
`view::editor::language_servers` run the same cleanup before removing old servers.
Stop keeps the registry's explicit stopped state; restart refreshes affected documents.
`view::language_servers` keeps selection and lifecycle state in private `registry.rs`,
with typed server/workspace and alternative-group keys. Active and manually
stopped instances have explicit states; workspace unavailability distinguishes
startup failure from an exit after initialization. Text synchronization conversions remain in `lsp::util`; conversions that apply
edits and snippets to documents live in `view::language_servers::edits`.

Configured feature selection maps `language_config::LanguageServerFeature` to
negotiated capabilities in `view::language_servers::capabilities`. The LSP client
has no language configuration dependency; individual request guards read protocol
capabilities directly.

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

`workspace::trust` owns trust policy, persisted grants/exclusions, and
workspace configuration hashing. `view::features::workspace_trust` owns each
editor's prompt history, pending requests, decisions, and service effects.
Requests arrive through `EditorEvent::WorkspaceTrust`; delivery and resolution
validate the owning editor, live workspace, current policy, and configuration
snapshot. Explicit decisions invalidate pending prompts and request a configuration
reload. Modal acceptance launches missing language servers; explicit trust commands
restart the current document's selected servers through
`Editor::restart_language_servers`, also used by `:lsp-restart`. Revoking or excluding
trust leaves running servers alone. `term` owns modal text, labels, and command adapters.

`view::features::auto_reload` owns external-change detection, polling, reload
decisions, split synchronization, VCS refresh, and ignore-filter refresh. Polls
use an editor-owned callback destination; weak ownership prevents closed or
replaced handlers from retaining or mutating an editor. Focus events call the
shared check for unwatched files.

`editor-config` owns watcher settings and diagnostic filters; rendering and watcher
behavior stay in `view`. `view::file_watcher` owns native watcher lifecycle, workspace and
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

`view::features::diagnostics::pull` owns pull-diagnostic requests, document and
inter-file debounce queues, server-requested retries, and report publication.
Result IDs and cancellation controllers live in feature state attached to each
document and are scoped per server. A provider refresh affects only that provider;
queued reports and retries validate their document snapshot and server attachment.
The terminal application calls shared refresh entry points for server progress and
diagnostic refresh requests. Diagnostic visibility during mode changes, panels,
and pickers stay in `term`.

`view::features::code_action_hint` owns debounced code-action availability checks
for each document/view. `view::action::code_actions_for_range` builds LSP requests
for hints, interactive actions, and actions on save. Documents retain hint state,
cancellation controllers, and their editor's scheduling handle. Selection,
document, diagnostic, and configuration changes invalidate pending results;
publication also checks the view, document snapshot, and server attachment.
Spelling findings provide hints even without an LSP server. Statusline and gutter
indicators and the picker remain in `term`; `view::save` owns save sequencing.

`view::action::Action` executes local actions synchronously. LSP actions retain
an `LspActionContext` from the document snapshot used to request them, including
server instance identity. Resolution runs asynchronously and returns through the
editor callback queue. Applying the result rechecks the document version, URI,
and server attachment; switching focus alone does not change the target. A resolve
or workspace-edit failure is reported and prevents the follow-up command. Existing
workspace-edit application can partially succeed before an error; it does not
roll back already-applied changes.

Feature modules group a feature's state, request policy, and lifecycle wiring.
Hover keeps those together in `features/hover.rs`. Inlay hints, document colors,
links, highlights, symbols, and code lenses each group cache/edit mapping in
`features/<feature>/state.rs` with request coordination in `requests.rs`.
`Document` owns typed feature state and delegates view cleanup and edit mapping to
it; `Handlers` binds each state to its editor's coordinator. Cache and scheduling
internals stay within the feature. Cache-only clears preserve replacement requests;
invalidation methods explicitly own request cancellation.
Document annotation types retain their intentional public facade in `document`.
All feature callers use `features` directly; there are no legacy handler aliases.
`handlers` composes the services and retains the LSP/DAP protocol bridges and
registry setup. Mandatory document mutation, syntax initialization, and text I/O
remain private document implementation modules.

`view::features::hover` owns multi-server requests and publishes typed
`EditorEvent::Hover` updates. The originating document owns cancellation by view;
edits, selection changes, focus loss, language/path changes, and view removal invalidate
pending work. Publication and frontend delivery both validate the snapshot.
`term` triggers hover and builds its popup from a resolved update.

`view::features::navigation` owns manual declaration, definition, type,
implementation, and reference requests. It aggregates supported servers and
retains each location's offset encoding. Document-owned cancellation and snapshot
checks cover edits, selection, focus, mode, language, URI, and server attachment.
Typed `EditorEvent::Navigation` updates validate again before the frontend opens
a picker or jumps. Selecting references applies first-server document highlights
in the shared editor, preserving the primary reference and shared LSP position conversion.

`view::features::document_links` prepares file navigation from cached links and
literal paths. Pending resolves retain the original editor, document, selection,
mode, and split binding. Document changes cancel work; queued updates validate
again before opening targets. Terminal commands share one opening policy and
convert file URIs with `to_file_path()` before filesystem access.

`view::features::inlay_hints` owns viewport selection, request deduplication,
position conversion, and annotation publication. Per-view cache keys include the
document version, URI, server instance, and buffered viewport. Pending requests
are reused until their inputs change; one split's response cannot mark another
split current. The frontend calls `refresh` on idle and renders cached annotations.

`view::features::signature_help` owns manual and automatic triggers, debounce
timing, server selection, cancellation, and response validation. Documents hold
weak scheduling handles to their editor's coordinator. Requests retain document,
view, selection, and server snapshots; newer requests and lifecycle changes
invalidate both queued callbacks and results awaiting presentation. Typed updates
arrive through `EditorEvent::SignatureHelp` and are resolved against the receiving
editor before display. `term` forwards mode and insertion events and owns popup
formatting, placement, signature navigation, and completion-menu overlap.

`view::features::completion` owns provider identities, text completions, LSP item
wrappers, and result batches. Response variants bind LSP items to their server,
and path/word variants carry text transactions without a second provider field.
The terminal presents and applies these items; `core` owns only the text edits.

`view::features::completion` owns completion items, LSP/path/word providers,
request debouncing, batching, incomplete-list refreshes, resolution, and
cancellation. Editor-owned callbacks publish typed `EditorEvent::Completion`
updates that validate their session again before presentation. Sessions retain
provider savepoints across normal typing and ghost previews; cancellation,
replacement, focus changes, and stale targets prevent old results from being
applied. `term` forwards input and owns menu filtering, navigation, documentation,
and preview/acceptance UI, including insert-repeat history.

`view::features::formatting` prepares explicit document and range-formatting tasks.
Frontends run each task and apply its typed update on the editor mutation path.
Updates validate their editor, document revision, URI, language loader, and original
split binding before applying edits or reporting failures. Range formatting checks
its provider is still attached and supports servers that only advertise range
formatting. Whole-document formatting commits one undo revision and restores the
cursor viewport; selection formatting preserves its existing history policy.

Manual formatting and accepted saves share the same document/configuration
snapshot checks. Their view lifetimes differ intentionally: manual formatting
requires its original split, while an accepted save continues on its document
through the save coordinator if that split closes or changes documents. Terminal
formatting commands only submit tasks and present current errors.

`loader::theme::Resources` owns theme file discovery, embedded default sources,
inheritance resolution, and TOML merging. It searches explicitly supplied roots
in priority order. Same-name inheritance can continue in a lower-priority root;
visited paths detect cycles. Style entries replace parent entries, while palette
entries retain the existing deeper merge. Sources remain lazy and uncached.

The `theme` crate interprets explicit resource sources and shared UI styles.
It has no dependency on editor state or LSP/DAP clients. Frontend symbol-kind
style selection lives in `term::ui::lsp`; theme selection is applied by the editor.

`theme::Loader` receives those resources and interprets their merged data
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

`clipboard` owns serializable provider settings, the backend interface, native
provider detection, and OS or command execution. It has no editor or terminal
dependency. `view::clipboard::Clipboard` holds live configuration access and an
application-supplied `clipboard::ClipboardBackend`. Registers own saved selections,
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

### Document mutation phases

`buffer::Buffer` owns rope text, text versions, pending undo
groups, and text history. `Document` retains per-view cursor snapshots, file
metadata, syntax, and editor services. History reads use shared references;
reading the current revision never temporarily removes the undo tree. Buffer
mutation returns an `AppliedChange` with the original text and changes. Text and
pending undo groups are complete before document effects or observers run; replay
uses the same publication path without recording history steps as new edits.

Text edits and undo groups belong to the document. `Document::apply` accepts a
`ChangeSet`, and `commit_changes`, `undo`, `redo`, `earlier`, and `later` do not
require a view. The core history stores text transactions and returns revision
identifiers with each history step. `view::editing` stores optional before/after
selection snapshots separately, keyed by those identifiers, and adapts interactive
commands to the same mutation path. Selection snapshots are shared by a document's
views to preserve cursor restoration across splits, even after an origin closes.

`Editor::apply_document_changes` applies an independent undo group and requires the
expected document version. It rejects stale same-length edits before committing
pending typing, synchronizes displaying splits before and after the operation,
and returns a typed rejection reason. Workspace edits and save formatting use
this operation. Direct document edits must be built from the current text.
Reload and save preparation also work on hidden documents; loading a buffer does
not create a selection in the focused split. Completion savepoints deliberately
retain their originating view and selection because they represent interactive
preview state.

Document mutations establish text, mapped selections, syntax, diagnostics, cached
annotation anchors, and snippet ranges before publishing a change. An explicit
transaction selection is installed before observers inspect the new state.
`SelectionDidChange` is delivered after `DocumentDidChange` for a text transaction
with an explicit selection; selection-only transactions publish only selection changes.

LSP synchronization is a required step on the mutation path. Normal edits queue
`didChange` before observers may request results; temporary completion previews
skip protocol synchronization. Server initialization queues configuration and
`didOpen` before feature observers run. Closing a document queues `didClose`
before close observers run. These guarantees do not depend on hook registration order.

### Service initialization and callbacks

`view::handlers::Handlers::new` constructs shared editor services from editor
configuration and an explicit callback destination. It registers editor events
and feature hooks through one central registration path, once per event registry.
Feature registrars are crate-private and called only from that composition path.
Mandatory document synchronization and snippet range tracking happen on the
mutation path before observer hooks. Registration includes filesystem configuration
updates. A frontend can initialize
these services without terminal setup. `term::handlers::register_hooks` installs
terminal input and presentation hooks through a separate central registration path,
once per event registry. Application-owned presentation subscriptions remain
separate and unregister when their application is dropped.

Configure a `Handlers` value before passing it to `Editor::new`. The editor owns
it and exposes `Editor::handlers()` for reading and scheduling work. All new
documents receive their service bindings through `Handlers::attach_document`
before syntax initialization or document-open events. A frontend cannot replace
owned handler fields directly.

Explicit editor methods replace running autosave, signature-help, completion,
and reload services. Autosave and signature-help replacement rebind existing
documents; their queued results retain the old owner identity and are rejected.
Completion replacement cancels pending/displayed work. Trust prompt reset clears
prompt ownership without changing trust policy. Terminal fixtures configure
services through `AppBuilder::with_handler_setup`, which delegates to
`Application::new_with_handler_setup` before the editor opens any documents.

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
Work that needs a compositor uses terminal job callbacks. Editor-only work uses
its owning editor's callback destination. Cancellation must also be checked when
a queued callback is applied, since canceling a request cannot retract a callback
already delivered to the frontend.

`view::editing::replace_selections` replaces a document's selections through one
transaction and commits pending edits to history. It takes a `Document`, a `View`,
and replacement text, preserving selection direction and the primary selection.
Text-only newline normalization lives in `core::line_ending`, shared with paste
and register replacement. The terminal adapter owns prompts, repeat recording,
mode transitions, and scrolling; prompted replacement and repeat use the same
editor operation.

`view::save` owns save options, whitespace/newline normalization, history
checkpoints, modified-document selection, save-all error policy, and the complete
pre-save sequence. `prepare` returns a request for one document; `submit` advances
its fixed pipeline: configured code-action kinds in order, resolution and edits,
formatting against the resulting document, then write submission. Code-action
snapshots validate document version/path and the attached server instance before
resolution and application. Resolve/edit failures retain the existing partial
success policy; request failures stop that save, and formatter failures still write.

`SaveProgress` distinguishes a queued write, a closed document, and a pending
`SaveTask`. Frontends poll tasks and apply `SaveUpdate`s on the owning editor's
mutation path, retaining every stage until completion during shutdown. The terminal
adapter translates those stages into exit-waiting jobs without interpreting save
policy. Switching or closing a view cannot redirect formatting or discard the
write. Save formatting always uses a document edit; its optional origin view is
used only to keep a displayed cursor in view. Save-all normalization does not
synthesize selections for hidden documents.
Formatting validates the text version, URI, language, and language-loader instance;
stale results are discarded while the current document text is written.

`save_all` prepares and submits each eligible document through a caller-supplied
callback. Preparation happens immediately before submission, so an immediate error
leaves later documents unprepared. `auto_save` uses this policy to enqueue writes
without formatting or code actions, skipping scratch buffers. `Editor::save` owns
the underlying write queue and document-saved events report write completion.

`view::save::WriteQueue` privately owns per-document admission channels, ordered
write futures, concurrent completion polling across documents, and pending-write
accounting. Closing a document rejects further writes but retains accepted work.
Both the event loop and `Editor::flush_writes` consume completions through the
same queue operation, including failed writes. Frontends can query
`Editor::has_pending_writes` without inspecting queue internals. Successful
completion applies the written revision and path through
`Editor::complete_document_write`; flushing drains all accepted writes before
returning the first failure. Required frontend jobs likewise finish their callbacks
and followups before reporting an error.

## Protocol and repository integrations

`lsp`, `dap`, and `vcs` own their transport or repository operations. They accept
explicit inputs; editor-specific state changes belong in `view`. The terminal
application decodes incoming protocol messages, delegates shared operations, and
handles frontend requests such as progress display and interactive prompts.

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
namespace. Public command entry points are collected in `commands.rs`; consumers
do not need to follow the feature modules' internal invocation namespaces.

Helpers stay with their feature and use narrow visibility for sibling callers.
For example, navigation owns jump recording, formatting owns its result callback,
and spelling and VCS expose range helpers to text-object selection. Picker
construction lives with the feature that supplies its content.

These modules adapt counts, registers, prompt phases, selections, and terminal
jobs. Document state, diagnostic storage, quicklists, and editor operations remain
in `view`; text algorithms remain in `core`; protocol and repository operations
remain in their integration crates. File commands use `view::save` for shared
preparation, batch policy, and save execution. Their terminal adapter drives typed
save stages as exit-waiting jobs. `view::features::auto_save` owns debouncing, insert-mode deferral,
focus-loss policy, and save submission through `view::save::auto_save`. Documents
schedule changes through weak handles to their editor's coordinator; callbacks
return through its explicit sender and reject replaced handlers, newer edits, and
disabled autosave. `term` forwards mode changes and focus loss and renders editor
errors. The command catalog remains static.

`keymap.rs` links commands to key combinations.


## Terminal rendering

`term::compositor` owns components, layers, input dispatch, and render orchestration.
`term::ui` owns widgets and conversions into Ratatui text and buffers. `tui` owns
the backend and terminal rendering integration; `ui-core` supplies shared value
types. A shared editor feature publishes state or a typed event, and the frontend
chooses the widget, labels, layout, and interaction.

The topmost component that declares cursor ownership controls the active cursor
for the frame. Decorative overlays preserve the owner below them. After rendering,
the owner returns a `tui::terminal::Cursor`: a software cursor with a hidden terminal
position for input methods, a native cursor with a shape, or no cursor. Only
`draw_with_cursor` applies that state to Ratatui and the backend. Backend shape
changes do not change visibility, and showing a cursor does not reset its shape.
The editor draws themed block cursors and secondary markers; active bars,
underlines, and prompt cursors use the terminal. When a prompt owns the cursor,
the editor keeps its previous position visible with a software block.

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

Built-in hooks live for the registry's lifetime. Use `register_owned_hook!` or
`register_dynamic_hook` for subscriptions whose lifetime belongs to a component,
application, or extension, and retain their `HookRegistration`. Dropping it
unsubscribes the callback and releases its captures once any active dispatch
finishes. Dispatch preserves registration order and runs callbacks without holding
the registry lock. Hooks added during dispatch participate in subsequent dispatches;
hooks removed before their turn are skipped. A callback already running may finish.

Frontend work uses an explicit `term::job::JobSender` from its owning `Jobs` or
`Application`, and shared editor work uses `view::callbacks::EditorCallbackSender`.
Capture that destination when starting background work; creating another application
never redirects an existing callback. Presentation hooks are owned by the
application and filter events by their editor's callback destination. `Jobs` constructs
one editor callback adapter and returns clones of it, so handles obtained through
separate accessor calls retain the same destination identity.

Working-directory state remains process-wide: `:cd` intentionally changes it,
and relative-path helpers and file watching continue to use that context. Shell
jobs capture their directory when submitted and pass it explicitly to the child
process; formatters similarly use the document directory or a captured fallback.
This avoids delayed work inheriting a later `:cd`; it does not provide independent
working directories for multiple editors in one process.

## Editor-owned signals

`event::Redraw` owns one frontend's coalescing notification and frame barrier. The
frontend creates it with `Jobs`; its `EditorCallbackSender` carries the same handle
to shared services. Custom callback queues pass that handle explicitly when
constructing their sender. Views, pickers, image decoders, and differ workers
retain their owner's handle. A frame waits only for that owner's computation guards;
those guards own their locks and can safely outlive the frontend.

`Handlers` owns a bounded status channel. `Editor::wait_event` receives its messages,
preserves severity, and schedules a redraw using the editor's existing frame-rate
limit. `StatusSender` clones keep their destination even on another thread; closed
destinations discard messages immediately. Asynchronous reports wait at most 10 ms
for an overloaded queue; synchronous reports use best-effort delivery and never wait
for queue capacity or require a Tokio runtime.

Synchronous event dispatch returns `HookErrors` after all observers run. Callers must
report them to the owner with `report_to`; standalone callers without a status UI
explicitly call `log`. Documents bind status delivery when attached to an editor;
standalone documents log observer failures. Initial VCS diff setup follows attachment
so workers never start with a different editor's redraw signal. No status or redraw
routing depends on the current process or Tokio runtime.

## Testing the boundaries

Headless tests in `crates/view/tests` exercise shared editor coordination with an
explicit callback queue. Use them for editor isolation, cancellation, stale
publication, configuration application, and lifecycle behavior. Terminal integration
tests in `crates/term/tests` exercise input, prompts, rendering adapters, and the
application event loop. The fake LSP executable in `crates/term/tests/fixtures`
provides controlled responses and gates for asynchronous protocol tests. Shared
helpers configure its command arguments, TOML language definitions, log paths,
and initialization gates; each feature suite keeps its response scenarios and
assertions explicit.

Place algorithm tests in the owning feature crate. Reuse existing fixtures and
keep assertions about observable behavior; moving code alone does not require a
new test. Changes to async ownership need coverage for queued callbacks as well
as in-flight requests.

## Workspace search

`search` owns directory traversal, regex matching, unsaved snapshot precedence,
and zero-based line and character-column match records. Callers supply roots,
ignore paths, traversal settings, cancellation, and a result sink. The terminal
owns the search prompt, picker, quicklist conversion, and navigation. Picker
invalidation stops traversal even when a query produces no matches. Buffer text
search and selection transformations remain in `core` and editor commands.

## Text I/O

`text-io` converts readers to ropes or strings and writes encoded ropes to async
streams. It owns encoding detection, BOM handling, and conversion fixtures with
no editor or protocol dependency. `view::document::file_io` retains file sniffing
and binary detection; documents retain file metadata, conflict handling, history,
and save coordination. Formatters, stdin loading, shell commands, and runtime
query inspection call the text conversion crate directly.

## Buffer snapshots

`buffer::Snapshot` captures shared rope text, its buffer identity and text version,
and the committed undo revision. Committing an undo group does not change the text
identity; edits and history replay invalidate it even when text becomes equal again.
Saving uses one snapshot for both bytes and revision accounting. Formatting, syntax,
and full spelling workers consume the captured text; their existing request wrappers
validate the same snapshot before publishing. Document IDs, paths, language/loaders,
protocol sessions, cancellation, and view bindings remain editor-level checks.

## Document/view state

Each document owns one `DocumentViewState` per bound view, containing its selection,
viewport position, and jump labels. Initialization preserves existing records;
removal drops all three together while feature coordinators cancel their own
requests. Selection and viewport anchors map through each edit before observers
run. Optional selection lookup and read-only iterators expose the state without
parallel maps. Revision-keyed cursor history remains document-owned so undo in
another split can restore the original selection after its originating view closes.

History navigation plans its steps without moving the current revision. Each
applied step advances the text, text version, and history cursor together before
publication, so intermediate snapshots and saves refer to the text they contain.
If an observer introduces a new edit or commits a different history branch during
replay, remaining stale steps stop without discarding that edit.

Committing an empty undo group clears its pending cursor snapshot as well as its
original text, preventing a later document-only edit from inheriting cursor state.

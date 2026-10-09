# Plugin API contract

The plugin interface is experimental. The prototype at commit `37776c06` uses
core WASM and JSON ABI 1. The production implementation uses the experimental `mitos:plugin@0.1.0`
component world and metadata protocol 3; the prototype transport is retired.
This contract defines editor behavior independently of the transport.

## Ownership and identifiers

An editor owns its plugin instances, callback channel, documents, and views.
An instance owns its registrations, jobs, resources, and transient state. A
replacement instance receives a new generation; callbacks and effects from the
old generation cannot affect the editor.

Document identity is session-local and distinct from a filesystem path. View
identity includes its allocation generation and is distinct from document
identity: one document can appear in several views. A view also has a binding
revision that changes when its document changes. Closing a document or view
invalidates its resources. Reopening a path does not revive a previous handle.

Every view-specific operation names its target view and document. The host never
substitutes the focused view for a missing, closed, or rebound target. A
document-only edit can target a hidden document; a selection/focus operation
requires the named live view.

## Text, selections, and transactions

Editor-facing offsets count Unicode scalar values, not UTF-8 bytes or UTF-16
code units. An edit replaces the half-open range `start..end`. The direction of
a selection is represented by its distinct anchor and head; its selected text
is `min(anchor, head)..max(anchor, head)`. Equal boundaries represent a cursor.
Conversions for language protocols occur at named host adapters.

Text version, view binding revision, and selection revision describe different
state. Changing a selection does not change the document's text version. A
response that depends on a selection carries the selection and binding
preconditions captured for that view. The host rejects stale responses instead
of overwriting newer input or guessing another target.

Edits within one edit action refer to its preceding projected text and must not
overlap. Later actions use the text after earlier edits; their version
precondition remains the original document version. Selection results refer to
the projected text at their position in the batch and are mapped through later
edits. Validate targets, revisions, offsets, output size, permissions,
and selections before mutation. Synchronize the actual target view's lazy
history/jump state before applying the transaction. One successful document
transaction creates one undo group, including its resulting selection.

Document transaction atomicity does not imply that opening documents, showing
UI, invoking processes, or updating several documents is one atomic operation.
Services return typed outcomes for those effects and specify their ordering.
Closed mutating `EditorRequest` services have an immediate native milestone:
a successful reply means that operation applied and its returned handles are live.
A later guest trap does not roll back a preceding scratch, navigation, register,
or settings call. Staged `Response` edit batches remain preflighted document
transactions; they do not turn a sequence of native service calls into one atomic
operation.

## Commands and events

Commands have a qualified plugin namespace, documentation, argument signature,
and completion behavior. Invocation context preserves the originating view,
arguments, count, register, and invocation source. User configuration precedence
and explicit command escaping remain consistent with native commands.
The manifest's argument signature bounds positional counts to 32. Completion
choices are bounded static literals; completion does not execute the guest or
delegate to filesystem, shell, or expansion services.

Post-command means completion of the public command invocation. Nested internal
implementation calls do not create duplicate completion events. Report success,
error, and cancellation distinctly; an accepted asynchronous command is not yet
a successful completed command. A custom command expands into canonical command
invocations, each tagged with the custom origin; the alias wrapper does not add
another completion event. Macro replay preserves its origin through the input
queue.

Native dispatch paths that deliberately hand ownership to another service report
`accepted` instead of success: LSP command/stop/restart, backend code-action
resolution, and external URL opening. These observations describe dispatch only;
consumers cannot use them as completion barriers. Tracked callbacks, queued jobs,
followups, and submitted writes retain invocation ownership until their actual
result. Async errors belong to that invocation, independently of later status
messages. A rebound origin keeps its original live document but omits the view.

Document lifecycle events originate in shared editor services, including
headless and blocking paths. A saved event follows a successful write and names
the written path and revision. The current buffer may already contain newer
text; consumers must not assume that the saved revision is the current one.
Failed writes do not emit saved success.

Insertion and document/terminal focus events preserve their originating
identities. Define paste, input-method, and programmatic insertion semantics at
the input adapter. Hooks receive owned observations; they do not borrow a mutable
command context. Edits made after an asynchronous insertion hook still require
revision checks against subsequent typing.

Events carry a sequence and mutation origin. Other plugins can observe
plugin-originated changes. The host controls a plugin's own-echo policy and
limits causal depth/work to prevent feedback loops. Dispatch ordering is
deterministic, but concurrent effects based on old state can conflict and must
return that conflict explicitly.

Lifecycle/control messages retain ordering. Coalescible invalidations identify
their document and view and permit reading the latest state. Ordered text deltas
must remain reconstructible. If a bounded queue cannot preserve continuity,
report a gap and provide list/query/resynchronization; never silently discard a
lifecycle event or block the editor waiting for a plugin.

## Host services and capabilities

Guests receive small metadata by default. Text access uses revisioned bounded
regions, chunks, or deltas. Startup, shutdown, and status-only commands do not
require a full document. The host retains owned snapshots where needed and
accounts for their lifetime and bytes.

Document/view/workspace, job, UI, and storage resources are scoped to the owning
editor, plugin, and generation. Resources contain host-owned state; handles do
not expose native pointers or internal object layouts. UI and slow services
return handles and later completion/results rather than occupying an instance
while awaiting user input or a long process.

Native prompts, pickers, and next-key requests retain the owning generation and
a fresh host token. At most eight frontend requests are outstanding across UI,
builtin groups, and keymaps. User acceptance, cancellation, timeout, loss of
origin, and unload each produce one targeted result; reusing a guest request ID
cannot revive an older native layer. Showing UI completes a command's presentation
milestone while the user result remains pending. Next-key timeouts are bounded
to 60 seconds. Rendering and cached previews do not call the guest.

Builtin composition admits a closed clipboard-free set of reviewed editor
commands. Its whole group checks capabilities, original focused view, text and
selection revisions, and count budgets before the first child. Total counts are
bounded to 128 and count-times-selection work to 8,192; later children recheck
remaining work because undo can change selections. This is not a rollback
transaction: a later failure reports the completed child count and typed error.
Clipboard operations use their explicit bounded asynchronous editor service.
Scoped keymaps can invoke only declared local plugin commands and restore the
remaining native/user mappings when removed. User configuration wins conflicts.

Every privileged request requires both a manifest declaration and a current user
grant. Enforce grants on direct calls and indirect command/provider/helper paths.
External filesystem, process, network, environment, and clipboard authority is
denied by default. Workspace trust cannot silently enlarge a plugin's grants.
Native tool execution is outside WASM memory isolation and needs a separate
explicit grant or platform isolation policy.

The runtime owns guest execution; editor/frontend adapters own their native
services. Editor callbacks never execute guests, compile modules, wait for I/O,
or hold editor references across guest calls. No native editor or syntax object
crosses the guest boundary.

## Limits, cancellation, and failures

Use bounded execution workers, per-plugin serialized stores, bounded mailboxes,
and quotas for guest memory/stack/tables, handles, input/output, retained
snapshots, host allocations, storage, jobs, subprocesses, and network results.
Control messages have reserved capacity; repeated notifications can coalesce.

Instruction fuel and epoch yielding do not replace total deadlines, cancellation,
or host-work limits. Cancellation revokes permission to apply effects immediately
and propagates to associated jobs/resources. Reload and target close cancel
obsolete work; late results cannot revive it. A guest trap or interruption can
leave guest state inconsistent, so the host discards that store when recovery
cannot be guaranteed.

Errors distinguish invalid requests, stale state, permission denial, resource
exhaustion, cancellation/deadline, unsupported interfaces, guest traps, and host
service failures. Return useful plugin-scoped diagnostics. A too-large payload
constructed by the host must not be blamed on a healthy guest.

## Reload, shutdown, and compatibility

Native resource changes during replacement trigger at most three asset-plan
rebases. A guest that has received `Shutdown` never resumes. If rebasing fails
after shutdown, the host retires the prior instances and requires an explicit
reload; their already validated registrations remain until replacement or final
cleanup. The editor reports this degraded state instead of restoring a shutdown
guest or publishing assets against an obsolete native configuration.

Declarative contributions contain bounded owned theme/query/snippet sources and
closed language profiles. Profiles use approved installed grammar identities;
formatter, language-server, debugger, or command configuration cannot enter through
them. Native associations win, and unloading restores native detection without
adopting a profile's base provider authority. Parsing, query validation, and
snippet expansion preflight occur off-thread. A delayed preparation must still
match the current native resource configuration before publication.

Developer inspection reads bounded owned metadata, diagnostic rings, and actual
queue/guest/application measurements. It never executes a guest. Failed preparation
remains visible alongside the surviving running generation.

Prepare and validate a replacement before switching generations. On switch,
cancel old work and release all owned commands, keymaps, UI, providers, jobs, and
handles. Restore surviving user/builtin/provider registrations according to
explicit precedence. Failed replacement preparation preserves the running
instance and reports a useful error.

On shutdown, complete accepted writes, deliver their lifecycle results, allow
bounded completion, quiesce mutation, then close instances and cancel remaining
jobs. Specify how hook failures and write failures affect shutdown. Post-save
mutation does not format the file that was already written; formatting before
save uses a separate bounded asynchronous protocol.

Manifest format, package version, public service/WIT version, and host release
support are separate contracts. Match declared requirements against the actual
artifact imports. Freeze published WIT worlds and support only a documented,
bounded compatibility window. Plugin-specific settings and protocol payloads
can still use JSON inside typed services.

# debugger

Session membership and selection history, thread/frame transitions, adapter lifecycle, and breakpoint
definitions and bindings. Models can be tested without an editor or terminal.

The session owner supplies asynchronous request results. `view` owns cancellation,
workspace trust, editor callbacks, and document navigation; `term` owns debugger UI.

Models depend on `dap-types`, without the asynchronous DAP client. The session
registry accepts the owner's generational key type.

Lifecycle tracks launch completion, breakpoint configuration, and shutdown separately.
An explicit shutdown rejects new commands; an adapter-terminated execution may
initialize a replacement while its previous cleanup callback is still queued.

Open documents share their source breakpoint definitions with the editor. Reads
return snapshots; edits map positions synchronously and advance definition revisions.
Bindings from older revisions cannot move or verify a current marker. Deleting
marked lines moves markers to the nearest surviving line; collisions retain the
oldest identity.

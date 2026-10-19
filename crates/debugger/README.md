# debugger

Session membership and selection history, thread/frame transitions, and breakpoint
definitions and bindings. Models can be tested without an editor or terminal.

The session owner supplies asynchronous request results. `view` owns cancellation,
workspace trust, editor callbacks, and document navigation; `term` owns debugger UI.

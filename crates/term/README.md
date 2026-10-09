# `term`

Owns the terminal application: startup, configuration composition, the event loop, keybindings, prompts, terminal clipboard output, and interactive UI rendering. Adapts terminal input to shared editor operations and presents their results.

Frame preparation publishes queued background results before presenting newly visible buffers with pending syntax. Presentation timing belongs here; shared syntax scheduling and document state belong to `view`.

Newly visible buffers share a 16 ms grace period for initial syntax. The frontend processes completion callbacks during that wait and renders as soon as syntax settles or the deadline expires. Subsequent frames for the same visible buffers redraw without another wait.

# `term`

Owns the terminal application: startup, configuration composition, the event loop, keybindings, prompts, terminal clipboard output, and interactive UI rendering. Adapts terminal input to shared editor operations and presents their results.

Frame preparation publishes queued background results before presenting newly visible buffers with pending syntax. Presentation timing belongs here; shared syntax scheduling and document state belong to `view`.

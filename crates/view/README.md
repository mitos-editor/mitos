# `view`

Owns the shared editor model and behavior: documents, views, editor state, editing and save operations, registers and configured clipboard access, shared icon data, theme application, and editor configuration. Integrates feature crates and coordinates workspace trust decisions, filesystem watching, background work, and protocol results across frontends.

Document-owned syntax requests share a bounded worker pool and validate cancellation, text version, and loader identity before publishing results. Frontends may debounce or cancel requests while retaining pending initialization for a later retry.

Stream decoding, encoding detection, BOM handling, and encoded writes belong to
`text-io`. Document file sniffing and binary detection live in `document/file_io.rs`;
the document model retains file lifecycle, history, and save coordination.

Theme parsing, palette resolution, and style lookup belong to the `theme` crate.

Clipboard provider definitions and native backends belong to `clipboard`. The
editor retains live configuration and register fallback behavior.

Text, versions, pending changes, and undo history are owned by the
`buffer` crate. Documents retain selection history, file lifecycle, and editor
services; reading a history revision no longer requires interior mutation.

`multibuffer` coordinates editable excerpts, source conflict checks, projection
history, background syntax, and source writes through the shared save queue.
Terminal commands, picker actions, and header rendering remain in `term`.

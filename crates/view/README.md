# `view`

Owns the shared editor model and behavior: documents, views, editor state, editing and save operations, registers and configured clipboard access, theme application, and editor configuration. Integrates feature crates and coordinates workspace trust decisions, filesystem watching, background work, and protocol results across frontends.

Document-owned syntax requests share a bounded worker pool and validate cancellation, text version, and loader identity before publishing results. Frontends may debounce or cancel requests while retaining pending initialization for a later retry.

Stream decoding, encoding detection, BOM handling, and encoded writes belong to
`text-io`. Document file sniffing and binary detection live in `document/file_io.rs`;
`document/file.rs` owns file identity, cached paths, encoding, saved state, and
open/save/reload operations. Its file state is private so path changes invalidate
derived caches together; the document model retains history and editor coordination.

Theme parsing, palette resolution, icons, and style lookup belong to the `theme` crate.

Clipboard provider definitions and native backends belong to `clipboard`. The
editor retains live configuration and register fallback behavior.

Contextual registers resolve through `Editor::read_register`; register storage
and clipboard synchronization operate independently of editor/view state.

Text, versions, pending changes, and undo history are owned by the
`buffer` crate. Documents retain selection history, file lifecycle, and editor
services; reading a history revision no longer requires interior mutation.

The `diagnostics` crate owns diagnostic values, provider identities, collections,
and edit mapping. `features/diagnostics` owns LSP conversion and publication for
push and pull providers. Frontends receive immutable diagnostic views; text
mutations map diagnostic anchors before notifying observers.

Debugger launch preparation is an editor operation by document ID. Templates,
request kinds, arguments, and workspace trust are checked before adapter startup;
terminal prompts retain the originating document across focus changes.

Language and dictionary overrides change through editor operations taking a
document ID. These operations own feature invalidation, language-server reopening,
and diagnostic/spelling refresh without requiring the document to be focused.

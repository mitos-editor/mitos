# `view`

Owns the shared editor model and behavior: documents, views, editor state, editing and save operations, registers and configured clipboard access, shared icon data, theme application, and editor configuration. Integrates feature crates and coordinates workspace trust decisions, filesystem watching, background work, and protocol results across frontends.

Document-owned syntax requests share a bounded worker pool and validate cancellation, text version, and loader identity before publishing results. Frontends may debounce or cancel requests while retaining pending initialization for a later retry.

Document stream decoding, encoding detection, BOM handling, and encoded writes live in `document/text_io.rs`. These helpers operate on readers, writers, and ropes; the document model retains file lifecycle, history, and save coordination.

Theme parsing, palette resolution, and style lookup belong to the `theme` crate.

Clipboard provider definitions and native backends belong to `clipboard`. The
editor retains live configuration and register fallback behavior.

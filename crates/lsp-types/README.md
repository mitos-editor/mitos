# Mitos LSP types

Vendored from [`mitos-editor/lsp-types`](https://github.com/mitos-editor/lsp-types)
at commit [`b6e8c0874628110e6b5addb5e237ae9aa68508e1`](https://github.com/mitos-editor/lsp-types/commit/b6e8c0874628110e6b5addb5e237ae9aa68508e1).
Protocol types and their wire-format tests are maintained in this workspace.
The crate retains its MIT license and upstream authorship.

Rust types for [Language Server Protocol 3.18](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.18/specification/), maintained for [Mitos](https://github.com/mitos-editor/mitos).
This fork started from upstream lsp-types 0.97.0. It requires Rust 1.85 or newer.

Standard protocol types are available with default features, including inline
completion, server-provided document content, folding-range refresh, formatting
multiple ranges, snippet edits, and completion-list merge rules. Client and server
capabilities still determine whether a feature can be used in a session.

## Feature flags

- `legacy-offset-encoding` enables clangd's non-standard `offsetEncoding`
  extension. Prefer the standard `general.positionEncodings` client capability
  and `positionEncoding` server capability.
- `proposed` remains a compatibility alias for `legacy-offset-encoding`.
  No standard LSP 3.18 definition requires this flag; the current official
  meta-model contains no proposed definitions.

## Migrating consumers

The protocol update changes some Rust representations:

- `Diagnostic.message` is `DiagnosticMessage`, supporting plain strings and
  `MarkupContent`. Use `.into()` for string literals or owned strings;
  `Diagnostic::new` and `Diagnostic::new_simple` continue to accept strings.
- `InlineCompletionItem.insert_text` is `OneOf<String, StringValue>`. Snippets
  use a `StringValue` with `kind: "snippet"`; `insert_text_format` is removed.
- `SignatureHelp.active_parameter` and `SignatureInformation.active_parameter`
  are `Option<Option<u32>>`: `None` omits the field, `Some(None)` sends `null`,
  and `Some(Some(index))` sends an index. Explicit null requires the client's
  `noActiveParameterSupport` capability.
- `DocumentFilter` distinguishes `Text(TextDocumentFilter)` and
  `NotebookCell(NotebookCellTextDocumentFilter)`. Text and notebook filter
  patterns accept `GlobPattern`, including relative patterns.
- Provider capabilities preserve registration options, selectors, identifiers,
  and work-done progress. Dynamic registration types are exported for each
  standard request. `documentSelector` is required and may be null.
- Semantic-token range support accepts a boolean or options object through
  `SemanticTokensRangeOptions`.
- Notebook synchronization uses `execution_summary_support`, matching the
  `executionSummarySupport` wire name. Code-lens client capabilities now have
  their own structure with `resolve_support`.
- Diagnostic request results contain final reports. Use
  `DocumentDiagnosticReportProgress` for document diagnostic progress;
  `DocumentDiagnosticReportPartialResult.related_documents` is a required map.
  `ProgressParamsValue::PartialResult` preserves partial-result payloads.
- `InitializeParams.process_id` and workspace diagnostic document versions use
  `i32`, matching the protocol's signed integer type. Deletion annotations belong
  to `DeleteFile.annotation_id`, rather than `DeleteFileOptions`.
- Execute-command registration exposes commands through
  `execute_command_options.commands`, without a duplicate `commands` field.

New optional fields are omitted when absent. Text document edits distinguish
plain, annotated, and snippet edits; semantic-token delta insertion data is a raw
integer array, so edits need not align to whole tokens.

For compatibility with JDT LS, snippet edits may omit `snippet.kind` on input;
serialization supplies the standard `"snippet"` discriminator.

## Contributing

Run `cargo test -p lsp-types --all-features` from the workspace root to check
protocol serialization, URI handling, and documentation examples.

Link protocol changes to the [specification](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.18/specification/)
and its [meta-model](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.18/metaModel/metaModel.json).
The protocol audit and wire fixtures use [this specification snapshot](https://github.com/microsoft/language-server-protocol/blob/ba1f1947af19dbf3c33fc763eadd7e7848f80766/_specifications/lsp/3.18/metaModel/metaModel.json).

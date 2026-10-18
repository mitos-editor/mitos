# `core`

Text data structures, transactions, Unicode, selections, and
language-independent editing algorithms. This crate depends on no parser,
runtime loader, workspace policy, editor state, or frontend.

Fuzzy scoring and reusable matcher buffers belong to `fuzzy`.

Language definitions, syntax trees, queries, and structural editing belong to
`language`. Highlight IDs and visual overlay traversal remain in `core::highlight`.

Text comparison belongs to `text-diff`.

History belongs to `buffer`; display layout belongs to `text-layout`. Completion
items and provider identities belong to `view::features::completion`.

Diagnostic values and edit-mapped collections belong to `diagnostics`.

# `core`

Text data structures, transactions, Unicode, selections, and
language-independent editing algorithms. This crate depends on no parser,
runtime loader, workspace policy, editor state, or frontend.

Language definitions, syntax trees, queries, and structural editing belong to
`language`. Highlight IDs and visual overlay traversal remain in `core::highlight`.

History belongs to `buffer`; display layout belongs to `text-layout`. Completion
items and provider identities belong to `view::features::completion`.

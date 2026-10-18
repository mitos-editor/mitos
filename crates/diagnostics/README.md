# `diagnostics`

Diagnostic values, provider identities, and sorted, edit-mapped collections.
Owns namespace-preserving replacement, bounded splicing, and diagnostic anchor
mapping over text changes. Positions use document character indices.

Editor features own protocol conversion, raw LSP caching, request lifetimes,
and publication. This crate has no editor, frontend, or async runtime dependency.

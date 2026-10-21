# `syntax-symbols`

Workspace syntax-symbol search with explicit roots, traversal options, language
resources, excluded open-file paths, cancellation, and a streaming result sink.
Owns query compilation, blocking file scanning, and parsed-file cache generations.
Prepared scans retain their cache generation across invalidation. Applications
choose the worker and adapt results to documents, navigation, and presentation.

The engine has no editor, event, protocol, frontend, or asynchronous runtime
ownership. Callers can use the same query pattern with their own syntax trees
and unsaved buffers before scanning remaining files.

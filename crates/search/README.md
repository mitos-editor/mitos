# `search`

Workspace text search with explicit roots, traversal options, unsaved rope
snapshots, cancellation checks, and a streaming result sink. Owns regex matching,
ignore traversal, binary-file handling, character-based match coordinates, and
capture expansion into text edits.
Does not depend on editor state, runtime paths, protocols, or UI. Applications
choose the worker that runs a search and render or navigate its results.

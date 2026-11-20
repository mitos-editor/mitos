# `buffer`

Owns rope text, text versions, pending undo groups, and text history. Applies
validated changes and returns their original text and change set. History replay
returns steps so callers can update derived state between individual changes.
Has no views, cursor history, configuration, filesystem operations, event dispatch,
syntax, protocol, or frontend dependencies. It depends only on `core` among local
crates. Document-level coordination and selection restoration belong to `view`.

Immutable snapshots pair shared rope text with its buffer identity, text version,
and committed revision. A text result remains current across a history commit,
but becomes stale after any edit or replay; equal text in another buffer cannot
validate the snapshot. Paths, protocol sessions, and view bindings remain caller
policy.

`projection` owns generic excerpt partitions, edit mapping, and line-range
merging. Source identities are supplied by the caller; document metadata,
source loading, conflict policy, and saving belong to `view::multibuffer`.

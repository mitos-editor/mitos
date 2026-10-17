# `buffer`

Owns rope text, text versions, pending undo groups, and text history. Applies
validated changes and returns their original text and change set. History replay
returns steps so callers can update derived state between individual changes.
Has no views, cursor history, configuration, filesystem operations, event dispatch,
syntax, protocol, or frontend dependencies. It depends only on `core` among local
crates. Document-level coordination and selection restoration belong to `view`.

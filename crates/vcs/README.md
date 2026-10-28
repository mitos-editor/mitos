# `vcs`

Owns version-control integration, repository status, change tracking, and diff-job
scheduling/publication. Pure text comparison, line interning, and indentation
heuristics belong to `text-diff`.

The optional `git` backend owns stage/unstage, stash, and commit workflows, repository
identity, literal pathspecs, and subprocess output limits. It uses Git's own
locking and filter semantics for writes. `view::features::git_operations` admits
operations against editor state and coordinates saves; terminal pickers only
present actions and drive their tasks to completion.

Commit snapshots bind a message to the reviewed index and HEAD. Git runs hooks
and signing normally; failed commits retain the editor's message buffer.

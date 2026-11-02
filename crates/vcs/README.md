# `vcs`

Owns version-control integration, repository status, change tracking, and diff-job
scheduling/publication. Pure text comparison, line interning, and indentation
heuristics belong to `text-diff`.

The optional `git` backend owns stage/unstage and stash workflows, branches,
linked worktrees, explicit remote operations, repository identity, literal
pathspecs, and subprocess output limits. Remote commands use configured names
and return redacted failures rather than network output. It uses Git's own
locking and filter semantics for writes. `view::features::git_operations` admits
operations against editor state and coordinates saves; terminal pickers only
present actions and drive their tasks to completion.

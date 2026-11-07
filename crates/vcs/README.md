# `vcs`

Owns version-control integration, repository status, change tracking, and diff-job
scheduling/publication. Pure text comparison, line interning, and indentation
heuristics belong to `text-diff`.

`DiffProviderRegistry` keeps compiled backends in priority order. Repository
queries use the first successful backend, while watch inputs are collected from
all backends. Git is currently the only implementation; additional backends such
as Mercurial or jj belong behind this dispatch boundary.

The optional `git` backend owns stage/unstage, stash, and commit workflows, branches,
linked worktrees, explicit remote operations, repository identity, literal
pathspecs, and subprocess output limits. Remote commands use configured names
and return redacted failures rather than network output. It uses Git's own
locking and filter semantics for writes. `view::features::git_operations` admits
operations against editor state and coordinates saves; terminal pickers only
present actions and drive their tasks to completion.

Commit snapshots bind a message to the reviewed index and HEAD. Git runs hooks
and signing normally; failed commits retain the editor's message buffer.

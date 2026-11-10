# `vcs`

Owns version-control integration, repository status, change tracking, and diff-job
scheduling/publication. Pure text comparison, line interning, and indentation
heuristics belong to `text-diff`.

Repository queries are stateless functions backed by the optional `git` feature.
Each inspection discovers current repository state and trust configuration.
Unavailable decorations and watches are empty; explicit comparison/status queries
report errors, including when Git support is disabled.

The optional `git` backend owns stage/unstage and stash workflows, branches,
linked worktrees, explicit remote operations, repository identity, literal
pathspecs, and subprocess output limits. Remote commands use configured names
and return redacted failures rather than network output. It uses Git's own
locking and filter semantics for writes. `view::features::git_operations` admits
operations against editor state and coordinates saves; terminal pickers only
present actions and drive their tasks to completion.

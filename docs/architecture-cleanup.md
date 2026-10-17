# Completing the editor architecture cleanup

The document/history, formatting, save queue, workspace-edit, spelling-resource,
and prompt-preview changes have landed on `main`. This plan closes the remaining
native editor ownership boundaries. GitHub records each PR's current status; this
document defines the remaining dependencies and endpoint. Lasting ownership
rules belong in [Architecture](architecture.md).

## Remaining merge tracks

| Change | Result | Prerequisite |
| --- | --- | --- |
| [#340](https://github.com/mitos-editor/mitos/pull/340) | Private document lifecycle and filesystem modules behind `Editor` | `main` |
| [#349](https://github.com/mitos-editor/mitos/pull/349) | Load unnamed scratch copies before registration; keep raw path mutation private | #340 |
| [#350](https://github.com/mitos-editor/mitos/pull/350) | Shared file-link navigation, correct file-URI conversion, and originating-context validation | `main` |
| [#345](https://github.com/mitos-editor/mitos/pull/345) | Explicit registry lifecycle states and focused registry/conversion modules | #342 |
| [#346](https://github.com/mitos-editor/mitos/pull/346) | Picker preview cache, workers, callbacks, and rendering under one private owner | `main` |

The cleanup PRs are independent except for their listed parents. Registry cleanup
builds on server alternatives [#342](https://github.com/mitos-editor/mitos/pull/342)
without including the feature implementation in its own diff. Picker ownership
builds directly on `main`; experimental interactive refactoring
[#343](https://github.com/mitos-editor/mitos/pull/343) can be reviewed separately.
After squash-merging a parent, rebase its child onto the resulting `main` commit
and retarget the base before merging. Preserve
each cleanup PR's focused commit rather than carrying its squashed parent back
into the diff.

[#297](https://github.com/mitos-editor/mitos/pull/297) and
[#302](https://github.com/mitos-editor/mitos/pull/302) are separate loader and
snippet foundations for [#130](https://github.com/mitos-editor/mitos/pull/130).
They are outside this native editor cleanup batch.

## Completion criteria

The batch is complete when these boundaries hold together:

- Text mutation, undo groups, and history navigation work without a view. Core
  history stores text; optional cursor snapshots are keyed by revision separately.
  Interactive commands share the document mutation path through `view::editing`.
  Asynchronous edits validate document versions before changing text or pending
  undo groups. Hidden buffers do not acquire synthetic selections.
- Document mutation establishes mapped state and final selections, then queues
  required protocol synchronization, then notifies observers. Hooks do not own
  mandatory mutation steps. Displaying splits synchronize their navigation state
  before and after independent document edits.
- Manual formatting owns provider execution and freshness checks in its shared
  feature. Explicit formatting retains its original split binding; accepted saves
  retain their document. Completion sessions retain their separate savepoint-based
  lifetime across typing and ghost previews. These are deliberate operation
  policies, not interchangeable guards.
- Save preparation, ordered pre-save stages, and write submission use the shared
  coordinator. The private write queue owns admission, per-document ordering,
  and completion counts. Accepted writes survive document closure; failed jobs or
  writes do not strand later work. Frontends drive required work to completion.
- Workspace edits have one shared operation boundary for document edits, resource
  operations, and failures. Precise document and file-open rejections reach callers.
  Processing stops at the failed operation; completed operations remain applied,
  matching the advertised LSP abort policy. Atomic filesystem rollback is outside
  this batch.
- Feature implementations and their document state have one owner in
  `view::features`. Spelling owns both loaded dictionaries and load coordination.
  Configuration and read methods expose feature operations rather than mutable
  resource maps. Existing dictionary snapshots remain shared across checks.
- Go-to-line preview state belongs to the command prompt that created it. Updates,
  cancellation, and confirmation respect the originating document, split binding,
  and text version. A stale prompt cannot attach itself to a replacement split or
  document. Shared `Editor` state contains no go-to-line preview selection.
- File-link discovery and LSP resolution live in the shared document-link feature.
  Pending navigation validates its originating editor, document version and URI,
  view binding, selection, mode, and attached servers before applying results.
  Selection, focus, binding, and document changes abandon obsolete resolution.
  Frontends own presentation and external launching. File URIs are decoded without
  query or fragment text; native binary paths retain their editor-opening behavior.
- Scratch copies finish loading and lose their source path before editor
  registration. They retain language and encoding, do not reuse or rename an open
  source buffer, and require an explicit save destination. Frontends change document
  paths through the editor lifecycle facade.
- Registry lifecycle state distinguishes active instances, explicit stops, startup
  failures, and exits. Server selection and process registration have one private
  module; protocol conversions have their existing public utility boundary.
- Picker previews own their cache, loading, rendering, cancellation, and publication
  callbacks in one private module. Dynamic-query scheduling remains separate; queued
  preview results cannot publish into a superseded picker selection.
- Background callbacks, status messages, redraws, and frame guards retain their
  owner's destination. Shared and frontend hooks install once. Replacing a running
  service invalidates previous results and rebinds documents through explicit
  editor methods.
- `Editor` remains the public facade. Its private document and filesystem modules
  group registration, loading, binding, closure, and path effects without adding a
  service framework or changing callers. Protocol handlers retain dispatch and
  lifecycle bridges rather than workspace-edit coordination.

## Verification before the final merge

Review the combined result, not only individual PR heads. Run strict workspace
Clippy, private Rustdoc, formatting, view unit and headless tests, and terminal
unit/integration tests. Preserve coverage for multiple editors, queued callbacks,
view rebinding, closed splits, hidden-document undo, pending typing groups,
formatter/configuration changes, failed writes, required-job followups, precise
workspace-edit failures, prompt and picker preview cancellation, scratch-copy
registration, URI decoding, and queued file-link resolution invalidation.

Run filesystem-watcher and workspace-trust checks with normal filesystem access.
Windows CI must verify write-completion path comparisons. Existing behavior tests
cover organizational moves; new tests describe durable behavior and rejection
contracts rather than asserting that a refactor happened.

Once these criteria pass together and the listed tracks merge, this batch is
finished. Further changes should address a concrete behavior or ownership problem
rather than extend this plan into another open-ended refactor.

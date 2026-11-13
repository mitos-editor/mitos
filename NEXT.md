# Readability backlog

Follow-up changes that reduce duplicated logic, redundant state, or unused API. Keep each change local, preserve the behavior called out below, and prefer the existing tests over introducing new abstractions. Check the two open PRs noted below before starting overlapping work.

## Cleanup

- [ ] **Trim unused theme icon configuration.** In [`crates/theme/src/icons.rs`](crates/theme/src/icons.rs), remove the unused `Ui`, `Gutter`, `Virtual`, `Statusline`, and `Indicator` subtrees (about 200 lines), private icon overrides that no configuration can set, and `Vcs` options that are always `Some`. Keep the file and directory `Theme` style patching that is used. Check theme loading and icon consumers after each removal; this is internal config/API cleanup with low behavioral risk.

- [ ] **Remove the unimplemented `ChangeSet::map`.** See [`crates/core/src/transaction.rs`](crates/core/src/transaction.rs). It is public, so check downstream use and decide whether the API removal needs a compatibility note. Preserve the position-mapping behavior implemented by existing methods; do not replace the stub with a second mapping abstraction.

- [ ] **Simplify case conversion.** Replace the one-use `CaseSwitcher` iterator in [`crates/core/src/transform.rs`](crates/core/src/transform.rs) with a direct output loop. Preserve selection mapping and Unicode case expansions where one character maps to multiple characters; add focused behavior coverage if it is missing.

- [ ] **Share selection line-range handling.** Use [`Selection::line_ranges`](crates/core/src/selection.rs) to remove duplicated line selection logic in [`crates/core/src/transform.rs`](crates/core/src/transform.rs) and [`crates/core/src/comment.rs`](crates/core/src/comment.rs). Preserve reversed and multiple selections and line-end behavior. Keep the selection line-range and comment/transform regression tests.

- [ ] **Unify line-ending detection.** The feature-specific implementations of `get_line_ending_of_str` in [`crates/core/src/line_ending.rs`](crates/core/src/line_ending.rs) can share one path: detect CRLF first, then use `LineEnding::from_char`. Retain the CR, LF, CRLF, and no-ending cases covered by the existing tests.

- [ ] **Simplify native clipboard command results.** In [`crates/clipboard/src/native.rs`](crates/clipboard/src/native.rs), replace the `execute_command` boolean plus `Option` return with the result shapes required by its fixed read and write callers. `MissingStdout` is unreachable for the current read command. Preserve custom-provider selection and the native provider's read/write behavior.

- [ ] **Remove unused debugger breakpoint wrappers.** In [`crates/debugger/src/breakpoints.rs`](crates/debugger/src/breakpoints.rs), remove `handle_event` and the test-only file-level `apply_response` wrapper if callers can use `handle_source_event` and `apply_source_response` directly. Keep source association and stale-revision coverage in the existing breakpoint tests.

- [ ] **Share file-watch filtering context.** In [`crates/file-watch/src/filter.rs`](crates/file-watch/src/filter.rs), factor the repeated longest-matching-root, global fallback, and depth rules in `ignore_path` and `ignore_path_rec` into a private helper. Preserve passing `is_dir = Some(true)` for ancestor directories while walking parents. Keep the root, global exclude, Git metadata, and ancestor tests in this module.

- [ ] **Reduce resolved indentation state.** In [`crates/language/src/indent.rs`](crates/language/src/indent.rs), replace the four counters in `Indentation` with a signed level plus alignment if that fully captures what `net_indent` consumes. Preserve capture and walk behavior, alignment, negative levels, and the indentation corpus in [`crates/language/tests/indent.rs`](crates/language/tests/indent.rs).

- [ ] **Make workspace folder access synchronous.** `Client::workspace_folders` in [`crates/lsp/src/client/workspace.rs`](crates/lsp/src/client/workspace.rs) only reads a mutex and does not await. Make it synchronous and update the caller in [`crates/term/src/application/lsp.rs`](crates/term/src/application/lsp.rs). Preserve pending-client capability behavior.

- [ ] **Share pane rectangle construction.** In [`crates/pane-layout/src/lib.rs`](crates/pane-layout/src/lib.rs), factor common axis rectangle construction while leaving balanced and weighted sizing policies separate. Preserve gaps, minimum sizes, and rounding; retain the layout recalculation tests, including rounding cases.

- [ ] **Consider reducing the `parsec` public API.** `token`, `take_until`, `reparse_as`, `filter`, and `left` in [`crates/parsec/src/lib.rs`](crates/parsec/src/lib.rs) have no production callers in the workspace; examples are their remaining consumers, while `snippets` uses the parser crate. Check whether these helpers are intended as external API before removing them (roughly 140 lines including examples). This is an API compatibility decision, not a mechanical deletion.

- [ ] **Give picker query state one owner.** Consolidate hierarchical query callbacks, the pending task, and refresh state in [`crates/term/src/ui/picker.rs`](crates/term/src/ui/picker.rs). Preserve synchronous results for an empty query, asynchronous filtering, and independent source lifetimes. Check overlap with [PR #1066](https://github.com/mitos-editor/mitos/pull/1066), which already unifies picker source updates, before implementation.

- [ ] **Share save option parsing.** The save option parsing in [`crates/term/src/commands/files.rs`](crates/term/src/commands/files.rs) and [`crates/term/src/commands/application.rs`](crates/term/src/commands/application.rs) can use one small shared path. Preserve the existing command and application behavior without adding a general parsing framework.

- [ ] **Reuse picker test setup.** Build on the shared helpers in [`crates/term/src/ui/picker/test.rs`](crates/term/src/ui/picker/test.rs) to reduce repeated `Context` setup in [`paging_tests.rs`](crates/term/src/ui/picker/paging_tests.rs) and [`scroll_tests.rs`](crates/term/src/ui/picker/scroll_tests.rs). Keep custom configuration, viewport dimensions, and test intent clear; do not create a broader fixture framework.

- [ ] **Flatten branch worker initialization.** In [`crates/vcs/src/operations/repository/branches.rs`](crates/vcs/src/operations/repository/branches.rs), replace nested initialization closures and the `Option::take().unwrap()` pattern with a direct loop modeled on [`crates/vcs/src/operations/history/walk.rs`](crates/vcs/src/operations/history/walk.rs). Preserve the ordering that closes the channel before publishing failure so callers can retry immediately. Separately, make `EventAccumulator::handle_event` synchronous in [`crates/vcs/src/diff/worker.rs`](crates/vcs/src/diff/worker.rs); it has no await point.

- [ ] **Make inlay hint request state explicit.** In [`crates/view/src/features/inlay_hints/state.rs`](crates/view/src/features/inlay_hints/state.rs), replace the optional key/controller plus completion boolean with an idle, pending, or completed state if it makes transitions clearer. Preserve pending and completed request deduplication, mapped cache behavior on edits, retry behavior, and stale-response rejection.

## Correctness fix

- [ ] **Fix the Windows thread-token output pointer in `faccess`.** In [`crates/stdx/src/faccess.rs`](crates/stdx/src/faccess.rs), `ThreadToken::new` currently passes a null `*mut HANDLE` to `OpenThreadToken` and dereferences it on success. Use a mutable `HANDLE` and pass `&mut handle`. Keep the change scoped to the directory ACL branch; ordinary files return earlier. Preserve impersonation restoration. The issue is source-verified but has not been runtime-tested on Windows, so validate that branch on Windows before claiming runtime coverage.

## Reconcile before starting

- DAP response channel cleanup overlaps [PR #887](https://github.com/mitos-editor/mitos/pull/887), which already includes a oneshot change among broader framing work. Check that PR before editing. If remaining work is needed, in [`crates/dap/src/transport.rs`](crates/dap/src/transport.rs) replace the capacity-one response channel with oneshot and move local reply metadata out of the serializable `Request` envelope. Preserve cancellation, wire ordering, and shutdown behavior.

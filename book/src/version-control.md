# Version control

Mitos provides Git change indicators, navigation, and commit information while you edit.

## Blame details

Run `:blame` to inspect the commit that last changed the current line. The command palette's `blame_line` command opens the same popup.

The popup shows the full commit ID, author name and email, date, and commit message. Inline blame does not need to be enabled.

| Key | Action |
| --- | --- |
| `y` | Copy the full commit ID to the clipboard |
| `Y` | Copy the commit message to the clipboard |
| `Ctrl-d`, `Page Down` | Scroll down |
| `Ctrl-u`, `Page Up` | Scroll up |
| `Esc` | Close |

Copy actions can be remapped under `[keys.blame]` using `copy_commit_id` and `copy_message`. Scrolling and closing use `[keys.popup]`.

Lines with uncommitted changes have no committed blame. Mitos reports "Not committed yet" for these lines.

## Inline blame

Show who last changed the current line with `Space B`. Press it again to hide
annotations. To enable annotations by default:

```toml
[editor.inline-blame]
show = "cursor-line"
format = "{author}: {title}"
```

Use `show = "all-lines"` for all visible lines, or `"never"` to hide them.
The toggle preserves a configured display mode; turning it on without a
chosen mode uses `"cursor-line"`. Blank lines and uncommitted changes have
no inline annotation. The `:blame` popup works whether annotations are on or off.

### Format

The default format is `{author}, {time-ago} • {title} • {commit}`.
Available placeholders are:

| Placeholder | Value |
| --- | --- |
| `{author}` / `{email}` | Author name / email |
| `{date}` / `{time-ago}` | Commit date / relative age |
| `{title}` / `{body}` | Commit title / body |
| `{commit}` | Abbreviated commit ID |

## Reviewing changes

Run `:show-change`, press `Space-i`, or choose `show_change` from the command palette
to inspect the change at the current line. The popup shows removed and added lines
with context and source line numbers. It compares `HEAD` with the current buffer,
including unsaved edits.

Use `Ctrl-d` / `Ctrl-u` or Page Down / Page Up to scroll. Left / Right scroll
horizontally. Close with Escape. Vertical scrolling and closing follow `[keys.popup]`;
horizontal scrolling follows `[keys.diff]`.

The changed-file picker (`Space-g`) has separate **staged** and **unstaged** columns.
Each path appears once, including files with both kinds of changes. Staged new files
are labeled `added`; files not yet added to Git are labeled `untracked`.

The preview shows a diff rather than the file's contents. Use Alt-Left / Alt-Right
to switch comparisons:

| Comparison | Before | After |
| --- | --- | --- |
| Staged | `HEAD` | Git index |
| Unstaged | Git index | File on disk |
| Unsaved | Git index | Current buffer, including unsaved edits |

For a file listed by Git, the unsaved comparison is available when it has a modified
text buffer.
The initial comparison shows unsaved edits when available, then unstaged changes,
then staged changes.

Alt-Up / Alt-Down scroll the preview. `Alt-o` opens the selected comparison in a
full-screen diff view, including in narrow terminals where the preview pane is hidden.
Enter opens the file; for a deleted file, it opens the diff instead. Opening a diff
captures a snapshot; refresh or reopen it to see subsequent changes.

### Full-screen diff view

Run `:diff` or choose `diff` from the command palette to review all changes in the
current file against `HEAD`, including unsaved edits. Use `:diff staged` for
`HEAD` versus the index, `:diff unstaged` for the index versus disk, or
`:diff buffer` for the index versus the open buffer. These comparisons also work
in a repository without an initial commit. For deleted files, open the diff from
the changed-file picker.

The gutter shows the original and new line numbers and a `+` or `−` change marker.
Added and removed rows have a background highlight across the view. A marker at
the left identifies the selected row; hunk headers separate changes with their
original source ranges. The heading shows the comparison, engine, and line counts.

| Key | Action |
| --- | --- |
| Up / Down, `k` / `j` | Select a row |
| Page Up / Page Down, Ctrl-u / Ctrl-d | Move by a page |
| Home / End | Select the first / last row |
| `p` / `n` | Previous / next hunk |
| Left / Right | Scroll horizontally |
| Alt-Left / Alt-Right | Switch comparison |
| Alt-d | Switch line / syntax engine |
| Enter | Jump to the selected location in the current file |
| `o` | Open the current file without using snapshot line numbers |
| `r` | Refresh the comparison |
| Escape, `q`, Ctrl-c | Return to the picker or editor |

Remap these actions under `[keys.diff]`. This view is read-only: editing keys and
pasted text do not change the underlying document. Staging and stash actions are
available after returning to the changed-file picker.

The view retains a snapshot while you review it. Refresh captures current Git
state and buffer contents; switching engines keeps the same source text. Enter
requires the current file to match the snapshot's new side, so staged changes or
external edits cannot send you to an unrelated line. Removed rows jump to their
deletion boundary. Use `o` to open a file whose contents differ. A deleted path is
never recreated by navigation. Opening a file returns to the editor; closing the
diff returns to the picker when it was opened there.

Picker controls can be remapped under `[keys.picker]`. Repository changes refresh
picker rows and previews while preserving the query and selected path. File watching
and polling follow the editor's file settings.

Reviewing never saves buffers or changes Git's index. Binary files, submodules,
and unresolved index conflicts show explanatory messages. Inputs are limited to
10 MiB and 200,000 lines per side; very large rendered diffs are truncated with a
notice.

### Syntax-aware review

Line comparison is the default. To use Tree-sitter for review views, set this in
`config.toml`:

```toml
[editor.diff]
engine = "syntax"
```

Use `"auto"` to choose syntax comparison for files with an available grammar and
line comparison for other files. A language can override the global choice in
`languages.toml`:

```toml
[[language]]
name = "rust"
diff-engine = "syntax"
```

Press **Alt-d** in the changed-file picker, full-screen diff, or a diff popup to switch between line
and syntax comparison for that review. The heading shows the active engine.
Remap this with `toggle_diff_engine` under `[keys.picker]` and `toggle_engine`
under `[keys.diff]`.

Syntax comparison uses the file's existing root grammar to match node kinds and
literal tokens across lines. Changed tokens are emphasized; matching text uses
the normal text color. This helps distinguish an expression change from surrounding
line wrapping. String contents, comments, and changes to syntax structure still
participate in comparison. Embedded languages use the root grammar's representation.

The original removed and added lines, source numbers, whitespace, and counts remain
visible. A `layout only` heading means the syntax matched while the text changed.
Diff gutters, hunk navigation, and `:restore-change` always use exact line hunks.
Syntax comparison does not infer semantic equivalence or identify moved code.

Missing or incompatible grammars, parse errors, and exhausted comparison budgets
fall back to line review. A notice explains the fallback; `auto` silently chooses
line mode for files without a language or grammar. Syntax inputs are limited to
512 KiB per side and 30,000 tree events per side, with a checked 300 ms budget for
the comparison. Parsing and traversal can be canceled; the bounded matching step
finishes before its result is accepted or discarded.

Toggling an open popup compares the same captured snapshots. Reopen it to pick up
new file contents or language configuration. Line and syntax presentations are
cached together, so switching back does not repeat matching. Picker previews reuse
their captured text when only the engine changes; file or language changes refresh
the comparison.

The standard build includes syntax comparison. Builds made without the
`syntax-diff` Cargo feature retain line review and report that the syntax provider
is unavailable when explicitly requested.

## Git picker actions

In the changed-file picker (`Space g`), press **Alt-a** for available Git actions.
Actions use the selected file's repository; with no selection, they use the picker root.

| Key | Action |
| --- | --- |
| `Alt-s` | Stage the selected file as it exists on disk |
| `Alt-u` | Unstage the selected file, keeping its working copy |
| `Alt-z` | Stash all repository changes, after confirmation |
| `Alt-l` | Browse stashes |

Staging does not save unsaved buffer edits. Staging and unstaging affect the entire
file, including both paths of a rename. Results refresh without losing the query
or jumping back to the first row.

Stashing saves staged, unstaged, and untracked files throughout the selected
repository, then restores its working tree to `HEAD`. Ignored files and changes
inside submodule working trees are excluded. Save or close modified buffers in
that repository first. Mitos suspends file saves while Git runs, reloads clean
buffers afterwards, and preserves edits made during the operation. Preserved
edits may need `:w!` to replace the changed disk version.

**Alt-l**, or the command palette's `git_stash_picker`, lists the newest 500
stashes. Press **Enter** to browse a stash's saved index, working tree, and
untracked files, then **Enter** on a file to review its immutable text diff.
Use **Alt-Enter** to keep a picker open while inspecting. In the stash picker,
**Alt-a** opens its action menu:

| Key | Action |
| --- | --- |
| `Alt-s` | Apply the stash and keep it |
| `Alt-i` | Apply the stash, restore its staged changes, and keep it |

Application requires confirmation and clean editor buffers. The selected stash
is identified by its commit, so later additions to the stash list cannot change
which stash gets applied. Git may leave conflicts to resolve; the stash is always
retained and Mitos refreshes the resulting files and index even on failure.

Git actions require the `git` executable on `PATH` and workspace Git trust
(`:workspace-trust`). Review and status browsing retain their existing trust policy.
Remap these actions in [`keys.git-picker` and `keys.stash-picker`](remapping.md#component-keys).

## Change navigation and restoration

Diff gutters compare the buffer with `HEAD`, including unsaved edits.

| Command | Default keys | Action |
| --- | --- | --- |
| `show_change` / `:show-change` | `Space-i` | Inspect the current change |
| `goto_next_change` / `goto_prev_change` | `]g` / `[g` | Select the next / previous change |
| `goto_first_change` / `goto_last_change` | `[G` / `]G` | Select the first / last change |
| `changed_file_picker` | `Space g` | Browse changed files within the workspace |
| `changed_file_picker_in_repository` | Unbound | Browse changed files throughout the repository |
| `:restore-change` | Unbound | Restore changes intersecting the selections from `HEAD` |

Restoring a change replaces whole intersecting hunks in the current buffer. Use `u` to undo it. The command does not save the file or modify Git's staging area.

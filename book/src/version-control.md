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
review buffer, including in narrow terminals where the preview pane is hidden.
The picker closes and is retained for `Space-'`. Initially Enter opens the file;
for a deleted file, it opens a review instead. After opening a review, the retained
picker opens further selections as reviews too. Ctrl-v / Ctrl-s open them in a
vertical / horizontal split. Opening a diff captures a snapshot; refresh or reopen
it to see subsequent changes.

### Review buffers

Run `:diff` or choose `diff` from the command palette to review all changes in the
current file against `HEAD`, including unsaved edits. Use `:diff staged` for
`HEAD` versus the index, `:diff unstaged` for the index versus disk, or
`:diff buffer` for the index versus the open buffer. These comparisons also work
in a repository without an initial commit. For deleted files, open the review from
the changed-file picker.

Reviews are ordinary editor buffers with read-only content. Use your normal
movement, search, selection, yank, wrapping, scrolling, and view commands.
`Space-'` reopens the last picker; select another file and use Ctrl-v / Ctrl-s to
review it alongside the first. `Ctrl-w v` / `Ctrl-w s` split the current review.
Each view has its own cursor and viewport. Reviews remain in the buffer picker,
and `Ctrl-o` returns through the normal jump history. Close a review with the
usual buffer or view commands.

The gutter shows the original and new line numbers and a `+` or `−` change marker.
Added and removed rows have a background highlight across the view. Hunk headers
separate changes with their source ranges. The buffer name shows the comparison,
engine, and line counts. Use the existing `[g` / `]g` change motions to move between
hunks (`[G` / `]G` for the first / last).

Review buffers use the source language's syntax colors with either comparison
engine. Removed lines use the original file's syntax; added and context lines use
the updated file's syntax. Multiline constructs and embedded languages keep their
full source context, even outside the displayed hunks. Theme changes apply to open
reviews. Files without a usable grammar retain plain diff highlighting.
Changed-token styles overlay the syntax colors; themes can use background or
modifier-only `diff.plus.word` / `diff.minus.word` styles to retain token foregrounds.

| Command | Action |
| --- | --- |
| Enter, `:diff-open-source` | Jump to the selected source location |
| `:diff-open-file` | Open the current source file without snapshot line numbers |
| `:diff-refresh` | Capture current Git state and buffer contents |
| `:diff-engine` | Switch line / syntax comparison for the captured source |
| `:diff staged`, `:diff unstaged`, `:diff buffer`, `:diff head` | Open another comparison |

The command palette also provides `diff_refresh`, `diff_toggle_engine`,
`diff_open_source`, and `diff_open_file`. Bind these through the normal editor
keymaps. `[keys.diff]` applies to compact diff popups.

Refreshing a review updates that buffer in all its views, mapping their selections
and viewport anchors through the change. Separate review buffers retain independent
snapshots. Engine switches reuse the captured source. Enter requires the live
source buffer to match the snapshot's new side; removed rows jump to their deletion
boundary. Use `:diff-open-file` when contents differ. Missing files are never
recreated by navigation.

Reviews have no file identity, unsaved modifications, or editable undo history.
Edits, reload, and writes (including `:write!` with a filename) are rejected.
Staging and stash actions remain in the changed-file picker.

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

Use `:diff-engine` in a review buffer or **Alt-d** in the changed-file picker or a
diff popup to switch between line and syntax comparison for that snapshot.
The review name or popup heading shows the active engine. Remap popup and picker
actions with `toggle_engine` under `[keys.diff]` and `toggle_diff_engine` under
`[keys.picker]`. Review buffer actions use the normal editor keymaps.

Syntax comparison uses the file's existing root grammar to match node kinds and
literal tokens across lines. Changed tokens are emphasized; matching text uses
the normal text color. This helps distinguish an expression change from surrounding
line wrapping. String contents, comments, and changes to syntax structure still
participate in comparison. Embedded languages use the root grammar's representation.

The original removed and added lines, source numbers, whitespace, and counts remain
visible. A `layout only` review name or popup heading means the syntax matched while the text changed.
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

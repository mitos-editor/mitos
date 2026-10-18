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
scrollable popup, including in narrow terminals where the preview pane is hidden.
Enter opens the file; for a deleted file, it opens the diff instead. Opening a diff
captures a snapshot; reopen it to see subsequent changes.

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

Press **Alt-d** in the changed-file picker or a diff popup to switch between line
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
Diff gutters, change navigation, and `:restore-change` always use exact line hunks.
Syntax comparison does not infer semantic equivalence or identify moved code.

Missing or incompatible grammars, parse errors, and exhausted comparison budgets
fall back to line review. A notice explains the fallback; `auto` silently chooses
line mode for files without a language or grammar. Syntax inputs are limited to
512 KiB per side and 30,000 tree events per side, with a checked 300 ms budget for
the comparison. Parsing and traversal can be canceled; the bounded matching step
finishes before its result is accepted or discarded.

Toggling an open popup compares the same captured snapshots. Reopen it to pick up
new file contents or language configuration. Picker previews refresh when their
language configuration or engine changes.

The standard build includes syntax comparison. Builds made without the
`syntax-diff` Cargo feature retain line review and report that the syntax provider
is unavailable when explicitly requested.

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

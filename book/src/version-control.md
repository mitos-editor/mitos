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

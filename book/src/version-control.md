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

For inline annotations and their display format, see [inline blame configuration](./editor.md#editorinline-blame-section). `toggle_inline_blame` (`Space-B`) toggles annotations without opening the detail popup.

## Changes

Diff gutters compare the buffer with `HEAD`, including unsaved edits.

| Command | Default keys | Action |
| --- | --- | --- |
| `goto_next_change` / `goto_prev_change` | `]g` / `[g` | Select the next / previous change |
| `goto_first_change` / `goto_last_change` | `[G` / `]G` | Select the first / last change |
| `changed_file_picker` | `Space-g` | Browse changed files within the workspace |
| `changed_file_picker_in_repository` | Unbound | Browse changed files throughout the repository |
| `:restore-change` | Unbound | Restore changes intersecting the selections from `HEAD` |

Restoring a change replaces whole intersecting hunks in the current buffer. Use `u` to undo it. The command does not save the file or modify Git's staging area.

The changed-file pickers include staged changes, unstaged changes, and untracked files. Each path appears once, including files with both staged and unstaged edits. Staged new files are labeled `added`; files not yet added to Git are labeled `untracked`.

# Version control

Use Git in Mitos to review changes, stage files, browse history, and see who last
changed a line.

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

Press `Space i` or run `:show-change` to inspect the change at the current line.
The popup compares `HEAD` with the current buffer, including unsaved edits, and
shows added and removed lines with context and line numbers.

Use `Ctrl-d` / `Ctrl-u` or Page Down / Page Up to scroll, Left / Right to scroll
horizontally, and Escape to close. Remap scrolling and closing under
`[keys.popup]`, and horizontal scrolling under `[keys.diff]`.

### Changed files

Press `Space g` to browse changed files. The picker has separate **staged** and
**unstaged** columns. Each path appears once, including files with both kinds of
changes. Staged new files are labeled `added`; files not yet added to Git are
labeled `untracked`.

The preview shows the selected file's diff. Use **Alt-p / Alt-n** to choose which
versions to compare:

| Comparison | Before | After |
| --- | --- | --- |
| Staged | `HEAD` | Staged version (Git index) |
| Unstaged | Staged version | File on disk |
| Unsaved | Staged version | Current buffer, including unsaved edits |

The unsaved comparison is available for listed files with a modified text buffer.
The preview starts with unsaved edits when available, then unstaged changes, then
staged changes. Alt-Left / Alt-Right also switch comparisons, but some terminals
use these keys for word movement. Use Alt-p / Alt-n in that case. These controls
change which versions are compared; they do not move between diff hunks.

Alt-Up / Alt-Down scroll the preview; Ctrl-t hides or shows it.

Enter opens the file. **Alt-o** opens the selected diff in a review buffer,
including in narrow terminals where the preview is hidden. For deleted files,
Enter also opens a review. After opening a review, `Space '` restores the picker
and Enter opens further selections as reviews. Use Ctrl-v / Ctrl-s to open them
in a vertical / horizontal split.

Picker controls can be remapped under `[keys.picker]`. Repository changes refresh
rows and previews while keeping the query and selected file.

### Review buffers

Run `:diff` to compare the current file with `HEAD`, including unsaved edits.
Choose another comparison with:

| Command | Comparison |
| --- | --- |
| `:diff head` | `HEAD` against the current buffer (the default) |
| `:diff staged` | `HEAD` against the staged version |
| `:diff unstaged` | Staged version against the file on disk |
| `:diff buffer` | Staged version against the current buffer |

These commands also work before the repository's first commit. For deleted
files, open the review from the changed-file picker.

Reviews open as read-only buffers. Use normal movement, search, selection, yank,
wrapping, and view commands. `Ctrl-w v` / `Ctrl-w s` split the current review;
`Space '` reopens the last picker so you can open another diff alongside it.
Reviews appear in the buffer picker. Use `Ctrl-o` to return through the jumplist
and the usual buffer or view commands to close them.

The gutter shows the old and new line numbers and a `+` or `−` marker. Added and
removed lines have a background highlight and keep the source language's syntax
colors. Hunk headers separate groups of changes. Use `[g` / `]g` to move between
hunks, or `[G` / `]G` for the first / last hunk.

Buffer names show the filename and comparison, for example
`editor.rs [diff: staged]`. The statusline shows change counts, the comparison
engine, and whether the review is loading.

| Command | Action |
| --- | --- |
| Enter, `:diff-open-source` | Jump to the selected source location |
| `:diff-open-file` | Open today's file without jumping to a diff line |
| `:diff-refresh` | Update the review with current Git state and buffer contents |

An open review keeps the contents it compared until you refresh it. Refreshing
updates every view of that review; other review buffers keep their own contents.
If the source has changed since opening the review, refresh before jumping to a
source line, or use `:diff-open-file` to open the current file.

The command palette provides `diff`, `diff_refresh`, `diff_open_source`, and
`diff_open_file`. Bind these through the normal editor keymaps. `[keys.diff]`
controls compact diff popups.

Reviewing does not save buffers or stage changes. Use the changed-file picker for
[Git actions](#git-picker-actions). Binary files, submodules, and unresolved index
conflicts show an explanation instead of a text diff. Very large files may be
rejected, or their diffs shortened with a notice.

### Syntax-aware review

Line comparison is the default. Syntax comparison highlights changed parts of
code while matching code across line breaks, which can make changes easier to
read when an expression has also been reformatted. Syntax highlighting is
available with either engine.

To enable syntax comparison, add this to `config.toml`:

```toml
[editor.diff]
engine = "syntax"
```

Use `"auto"` to choose syntax comparison for supported languages and line
comparison for other files. A language can override the global choice in
`languages.toml`:

```toml
[[language]]
name = "rust"
diff-engine = "syntax"
```

Reviews use the configured engine, with language settings taking precedence over
`editor.diff.engine`. After changing configuration, refresh or reopen an existing
review to use the new setting. The review statusline or popup heading shows the
active engine.

Both engines show the original added and removed lines, whitespace, line numbers,
and counts. A `layout only` label means the syntax matched despite changes to the
text's layout. Comments and string contents still take part in comparison.
Change navigation and `:restore-change` use line changes with either engine.
Syntax comparison does not determine whether two pieces of code behave the same
or identify moved code.

Syntax comparison needs an available language grammar. If the grammar is missing,
the code cannot be parsed, or the comparison is too large, Mitos falls back to
line comparison. A notice explains the fallback; `auto` uses line comparison
without a notice for unsupported languages.

## Git picker actions

Use these shortcuts directly in the changed-file picker (`Space g`). The picker
stays open after staging or unstaging, and stashing asks for confirmation.
Actions use the selected file's repository, or the picker's repository when no
file is selected.

| Key | Action |
| --- | --- |
| `Alt-s` | Stage the selected file as it exists on disk |
| `Alt-u` | Unstage the selected file, keeping its working copy |
| `Alt-z` | Stash all repository changes, after confirmation |
| `Alt-l` | Browse stashes |

The picker shows these shortcuts below the file list. **Alt-a** opens the
action menu as an alternative. Remapped shortcuts appear in the help too.

Staging does not save unsaved buffer edits. Staging and unstaging affect the
entire file, including both paths of a rename. The picker keeps its query and
selected file after each action.

Stashing saves staged, unstaged, and untracked files throughout the selected
repository, then restores its working tree to `HEAD`. It excludes ignored files
and changes inside submodules. Save or close modified buffers in that repository
first. File saves pause while Git runs. Mitos reloads clean buffers afterwards
and keeps edits made during the operation; those edits may need `:w!` to replace
the changed file on disk.

### Stashes

Press **Alt-l** in the changed-file picker or choose `git_stash_picker` from the
command palette to browse the newest 500 stashes. Press Enter to browse a stash's
saved staged version, working tree, and untracked files, then Enter on a file to
review its saved changes. Alt-Enter keeps a picker open while inspecting a selection.

In the stash picker, use these shortcuts directly. **Alt-a** also offers the
same actions in a menu:

| Key | Action |
| --- | --- |
| `Alt-s` | Apply the stash and keep it |
| `Alt-i` | Apply the stash, restore its staged changes, and keep it |

Applying a stash requires confirmation and saved or closed modified buffers.
Git may leave conflicts to resolve. The stash is kept even if applying it fails,
and Mitos refreshes the resulting files and staged changes.

Git actions require the `git` executable on `PATH` and workspace Git trust
(`:workspace-trust`). Remap these actions under
[`keys.git-picker` and `keys.stash-picker`](./remapping.md#component-keys).

## Change navigation and restoration

Diff gutters compare the buffer with `HEAD`, including unsaved edits.

| Command | Default keys | Action |
| --- | --- | --- |
| `show_change` / `:show-change` | `Space i` | Inspect the current change |
| `goto_next_change` / `goto_prev_change` | `]g` / `[g` | Select the next / previous change |
| `goto_first_change` / `goto_last_change` | `[G` / `]G` | Select the first / last change |
| `changed_file_picker` | `Space g` | Browse changed files within the workspace |
| `changed_file_picker_in_repository` | Unbound | Browse changed files throughout the repository |
| `:restore-change` | Unbound | Restore changes intersecting the selections from `HEAD` |

Restoring a change replaces whole intersecting hunks in the current buffer. Use
`u` to undo it. The command does not save the file or change Git's staging area.

## Git history

Run `:file-history` to browse commits that changed the current file, following
renames detected by Git. The preview shows each selected commit's diff against
its first parent. Use Ctrl-t to hide or show the preview, Alt-Up / Alt-Down to
scroll.

Press **Enter** to open the diff in a review buffer. Ctrl-v / Ctrl-s open it in a
vertical / horizontal split. `Space '` restores the picker with its query and
selection, so you can open another comparison alongside it.

**Alt-a** opens actions for the selected commit:

- Open the version of the file saved in that commit.
- Compare it with the file buffer you started from, including unsaved edits.
- Compare a merge commit with a different parent.

File history follows the first parent of merge commits. To browse commits from
merged branches as well, use repository history.

### Repository history

Run `:git-history` (alias `:vcs-history`) to browse repository commits. Select a
commit, then a changed file to preview and review its diff. Merges compare with
their first parent by default; use Alt-a to choose another parent.

In the changed-file browser, **Alt-h** returns to the commit browser and restores
its query and selection. Alt-a also offers historical versions and comparisons
with an open buffer for that file. `Space '` restores the last picker after
opening a review.

Repository history uses the current file's repository, or the current directory
when no file is open. Both history commands are also available in the command
palette as `git_file_history` and `git_history`.

### Older versions and comparisons

Historical versions open in read-only buffers with syntax highlighting. In a
historical diff, Enter on an added or unchanged line opens that commit's version;
on a removed line it opens the parent version. `:diff-open-file` opens today's
file instead.

An open historical comparison keeps the revisions and any unsaved buffer text
from when you requested it. `:diff-refresh` reloads that same comparison. Open a
new comparison to include newer edits. Browsing history does not check out
revisions or change files or staged changes.

History loads 100 commits at a time. Select **Load 100 older commits…** to continue;
clear the query if that row is hidden by your filter. Each browser shows at most
5000 commits, and filtering searches only the commits already loaded.

Remap history actions under `[keys.history-picker]`, which inherits shared
picker bindings. See [Component keys](./remapping.md#component-keys).

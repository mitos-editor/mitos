# Version control

Press `Space g` to choose a Git action.

| Key or command | Action |
| --- | --- |
| `Space g s`, `:vcs-changes` | Browse changed files |
| `Space g d`, `:vcs-diff` | Review changes in the current file, including unsaved edits |
| `Space g i`, `:vcs-show-change` | Inspect the change at the cursor |
| `Space g f`, `:vcs-file-history` | Browse commits that changed the current file |
| `Space g l`, `:vcs-history` | Browse repository commits and their changed files |
| `Space g b`, `:vcs-blame` | Show the commit that last changed the current line |
| `Space g t`, `:vcs-toggle-inline-blame` | Show or hide inline blame |
| `Space g S`, `:vcs-stashes` | Browse stashes |
| `Space g a` | Choose a Git command with completion |

`s` means status, `S` stashes, `f` file history, and `l` log. `Space g c`,
`Space g h`, and `Space g H` are aliases for status, file history, and log.

For other Git actions, press `Space g a` to open the command prompt with `vcs-`
already entered. Use completion to choose a command and enter its arguments,
then press `Enter` to run it. You can also find these commands by searching for
`vcs-` in the command palette (`Space ?`).

Existing commands such as `:diff`, `:file-history`, and `:git-history` still work.
`Space i` also inspects a change, and `Space B` toggles inline blame.

Previously, `Space g` opened changed files directly; that action is now
`Space g s` (or `Space g c`). To open changed files directly again, replace the
Git menu with this binding in `config.toml`:

```toml
[keys.normal.space]
g = "changed_file_picker"

[keys.select.space]
g = "changed_file_picker"
```

Staging, stashes, and history browsing require the `git` executable on `PATH`.

## Reviewing changes

`HEAD` is your checked-out commit, usually the latest commit on the current
branch. Git's staging area holds the changes you have selected for your next
commit. A hunk is a group of changed lines. Reviews compare two versions of a
file without saving it or changing the staging area.

### Changed files

Press `Space g s` to browse changes within the workspace. To browse the whole
repository, run `:vcs-repository-changes`.

The picker has separate **staged** and **unstaged** columns. Each path appears
once, including files with both kinds of changes. New files are labeled `added`
when staged and `untracked` before they are added to Git.

The preview shows the selected file's diff. Use `Alt-p` / `Alt-n` to choose which
versions to compare:

| Comparison | Before | After |
| --- | --- | --- |
| Staged | `HEAD` | Staged version (staging area) |
| Unstaged | Staged version | File on disk |
| Unsaved | Staged version | Current buffer, including unsaved edits |

The unsaved comparison is available for listed files with a modified text buffer.
The preview starts with unsaved edits when available, then unstaged changes, then
staged changes. `Alt-Left` / `Alt-Right` also switch comparisons, but some
terminals use those keys for word movement.

| Key | Action |
| --- | --- |
| `Enter` | Open the file |
| `Alt-o` | Open the selected diff in a review buffer |
| `Alt-Up` / `Alt-Down` | Scroll the preview |
| `Ctrl-t` | Hide or show the preview |

For deleted files, `Enter` opens a review. `Alt-o` also works when the preview is
hidden. After opening a diff, `Space '` restores the picker, and further
selections open as reviews. Use `Ctrl-v` / `Ctrl-s` to open them in a vertical /
horizontal split.

Results refresh as files change, keeping the query and selected file. Remap these
controls under `[keys.git-picker]`; shared navigation uses `[keys.picker]`.
See [Component keys](./remapping.md#component-keys).

### Review buffers

Run `:diff` to compare the current file with `HEAD`, including unsaved edits.
Choose another comparison with:

| Command | Comparison |
| --- | --- |
| `:diff`, `:diff head` | `HEAD` against the current buffer |
| `:diff staged` | `HEAD` against the staged version |
| `:diff unstaged` | Staged version against the file on disk |
| `:diff buffer` | Staged version against the current buffer |

These comparisons also work before the repository's first commit. For deleted
files, open the review from the changed-file picker.

Reviews are read-only buffers. Use normal movement, search, selection, yank,
wrapping, and view commands. Reviews appear in the buffer picker; use the usual
buffer or view commands to close them. `Ctrl-o` returns through the
[jumplist](./jumplist.md).

The gutter shows old and new line numbers and a `+` or `−` marker. Added and
removed lines have background highlights and the source language's syntax colors
when available. Hunk headers separate groups of changes. Use `[g` / `]g` to move
between hunks, or `[G` / `]G` for the first / last hunk.

Buffer names show the filename and comparison, for example
`editor.rs [diff: staged]`. Separate reviews of the same comparison receive
numbered suffixes. The statusline shows change counts, the active comparison
engine, and whether the review is loading.

| Command | Action |
| --- | --- |
| `Enter`, `:diff-open-source` | Jump to the selected source location |
| `:diff-open-file` | Open the current file without jumping to a diff line |
| `:diff-refresh` | Retry loading the current review |

`Enter` can jump into the current file only when it matches the updated version
in the comparison. Removed lines jump to their deletion location in that file.
If the file differs from the version being reviewed, use `:diff-open-file`
without a line jump.

Current-file reviews update automatically as you edit, stage changes, or change
files outside Mitos. Updates apply to every view of each affected review. Buffer
comparisons use the file on disk if you close its source buffer, and follow the
buffer again when you reopen it.

External updates follow your [file watching and polling settings](./configuration.md#editorfileswatch).
Use `:diff-refresh` to retry a failed read or update a review when watching and
polling are disabled. [Historical comparisons](#older-versions-and-comparisons)
keep their selected revisions and captured buffer text, including when refreshed.

To review two files side by side:

1. Press `Space g s`, select a file, and press `Alt-o`.
2. Press `Space '` to restore the picker and select another file.
3. Press `Ctrl-v` to open the second diff in a vertical split.

Use `Ctrl-w v` / `Ctrl-w s` to split an existing review. The command palette also
lists the diff commands.

Binary files, submodules, and unresolved merge conflicts show an explanation
instead of a text diff. Very large files may be rejected, or their diffs shortened
with a notice.

### Inspecting the current change

Press `Space i` or run `:show-change` to inspect the change at the cursor in a
popup. It compares `HEAD` with the current buffer, including unsaved edits, and
shows added and removed lines with context and line numbers.

Use `Ctrl-d` / `Ctrl-u` or Page Down / Page Up to scroll, Left / Right to scroll
horizontally, and `Esc` to close. Remap scrolling and closing under `[keys.popup]`,
and horizontal scrolling under `[keys.diff]`.

### Syntax-aware review

Line comparison (`engine = "line"`) is the default. Syntax comparison highlights
changed parts of code while matching code across line breaks, which can make
reformatted changes easier to read. Review buffers keep syntax highlighting with
either engine.

To enable syntax comparison, add this to `config.toml`:

```toml
[editor.diff]
engine = "syntax"
```

Use `"auto"` to choose syntax comparison for supported languages and line
comparison for other files. Override the global choice for a language in
`languages.toml`:

```toml
[[language]]
name = "rust"
diff-engine = "syntax"
```

Language settings take precedence over `editor.diff.engine`. Run `:config-reload`
after editing configuration. Open review buffers apply the new setting
automatically; reopen a popup to update it. The review statusline or popup heading
shows the active engine.

Both engines show the original added and removed lines, whitespace, line numbers,
and counts. A `layout only` label means the syntax matched while the text's
layout changed. Comments and string contents still take part in comparison.
Syntax comparison does not check whether code behaves the same; moved blocks
still appear as removed and added text.

If a grammar is missing, code cannot be parsed, or a comparison is too large,
Mitos falls back to line comparison with a notice. In `auto` mode, unsupported
languages use line comparison without a notice.

## Change navigation and restoration

In editable buffers, diff gutters compare your text with `HEAD`, including
unsaved edits. Use a count such as `3]g` to jump forward three changes.

| Key or command | Action |
| --- | --- |
| `[g` / `]g` | Select the previous / next change |
| `[G` / `]G` | Select the first / last change |
| `:restore-change` | Restore changes intersecting the selections from `HEAD` |

Restoring replaces whole intersecting hunks in the current buffer. Use `u` to
undo it. The command does not save the file or change Git's staging area.
Navigation and restoration use line changes with either comparison engine.

## Git picker actions

Use these shortcuts directly in the changed-file picker (`Space g s`). Actions
affect the selected file's repository, or the picker's repository when no file is
selected. Staging, unstaging, stashing, and applying stashes require
[workspace trust](./workspace-trust.md). Use `:workspace-trust` to approve the
workspace.

| Key | Action |
| --- | --- |
| `Alt-s` | Stage the selected file as it exists on disk |
| `Alt-u` | Unstage the selected file, keeping its working copy |
| `Alt-z` | Stash all repository changes, after confirmation |
| `Alt-l` | Browse stashes |

The picker shows these shortcuts below the file list, including any remappings.
`Alt-a` offers the same actions in a menu. Staging and unstaging keep the query
and selected file.

Staging does not save unsaved buffer edits. Staging and unstaging affect the
entire file, including both paths of a rename.

Stashing saves staged, unstaged, and untracked files throughout the repository,
then restores its working tree to `HEAD`. Ignored files and changes inside
submodules are excluded. Save modified buffers in that repository first.

While stashing or applying a stash, file saves in that repository are temporarily
blocked. Mitos updates open buffers afterwards. Edits you make during the
operation are kept; saving them may require `:w!` because Git changed the file
on disk.

### Stashes

Press `Alt-l` in the changed-file picker or choose `git_stash_picker` from the
command palette to browse the newest 500 stashes. Press `Enter` to browse a
stash's saved staged version, working tree, and untracked files, then `Enter` on
a file to review its saved changes. `Alt-Enter` keeps the picker open.

In the stash picker, use these shortcuts directly or choose an action with
`Alt-a`:

| Key | Action |
| --- | --- |
| `Alt-s` | Apply the stash and keep it |
| `Alt-i` | Apply the stash, restore its staged changes, and keep it |

Applying a stash asks for confirmation. Save modified buffers in that repository
first. Git may leave conflicts to resolve; the stash is kept even if applying it
fails. Mitos refreshes the resulting files and staged changes.

Remap Git and stash actions under `[keys.git-picker]` and `[keys.stash-picker]`.
See [Component keys](./remapping.md#component-keys).

## Git history

Use history to review older changes or compare a past version with your current
buffer. Browsing history does not check out revisions or change files or staged
changes. Choose `git_file_history` or `git_history` from the command palette
(`Space ?`), or use the commands below.

### File history

Run `:file-history` to browse commits that changed the current file, following
renames detected by Git. The preview shows the selected commit's diff against
its first parent. `Ctrl-t` hides or shows the preview; `Alt-Up` / `Alt-Down`
scroll it.

Press `Enter` to open the diff in a review buffer. `Ctrl-v` / `Ctrl-s` open it in a
vertical / horizontal split. `Space '` restores the picker with its query and
selection, so you can open another comparison alongside it.

Use `Alt-a` to open the file's version at that commit or choose another parent
of a merge commit. When browsing from an editable file buffer, it also offers a
comparison with that buffer, including unsaved edits.

File history follows the first parent of a merge: the previous commit on the
branch that received the merge. Use repository history to see individual commits
from merged branches.

### Repository history

Run `:git-history` (alias `:vcs-history`) to browse commits leading to `HEAD`,
including commits from merged branches. It uses the current file's repository,
or the current directory when no file is open. Run the command again after
switching branches to browse the new branch.

Select a commit, then a changed file to preview and review its diff. Merges
compare with their first parent by default; use `Alt-a` to choose another parent.
`Alt-a` also offers historical versions and comparisons with an open buffer for
that file.

In the changed-file browser, `Alt-h` returns to the commit browser with its query
and selection. `Space '` restores the last picker after opening a review.

### Older versions and comparisons

Historical versions open in read-only buffers with syntax highlighting. In a
historical diff, `Enter` on a removed line opens the original version; on an added
or unchanged line it opens the updated version. When comparing with unsaved
edits, this is the captured buffer text. `:diff-open-file` opens the current file
instead.

Refreshing a historical comparison keeps the same revisions and any buffer text
from when you opened it. Start a new comparison to include newer edits.

History loads 100 commits at a time, up to 5000 per picker. Select
**Load 100 older commits…** to continue; clear the query if that row is hidden by
your filter. Filtering searches only commits already loaded.

Remap history actions under `[keys.history-picker]`, which inherits shared picker
bindings. See [Component keys](./remapping.md#component-keys).

## Blame details

Run `:blame` or choose `blame_line` from the command palette to inspect the commit
that last changed the current line. The popup shows the full commit ID, author
name and email, date, and commit message. Lines with uncommitted changes report
"Not committed yet".

| Key | Action |
| --- | --- |
| `y` | Copy the full commit ID to the clipboard |
| `Y` | Copy the commit message to the clipboard |
| `Ctrl-d`, `Page Down` | Scroll down |
| `Ctrl-u`, `Page Up` | Scroll up |
| `Esc` | Close |

Copy actions use `[keys.blame]` with `copy_commit_id` and `copy_message`.
Scrolling and closing use `[keys.popup]`.

## Inline blame

Show who last changed the current line with `Space B`. Press it again to hide
annotations. To show them by default, add this to `config.toml`:

```toml
[editor.inline-blame]
show = "cursor-line"
format = "{author}: {title}"
```

Use `show = "all-lines"` for all visible lines, or `"never"` to hide them.
Turning annotations back on restores the previous mode, defaulting to
`"cursor-line"`. Blank lines and uncommitted changes have no inline annotation.
The `:blame` popup works whether annotations are shown or hidden.

### Format

The default format is `{author}, {time-ago} • {title} • {commit}`.
Available placeholders are:

| Placeholder | Value |
| --- | --- |
| `{author}` / `{email}` | Author name / email |
| `{date}` / `{time-ago}` | Commit date / relative age |
| `{title}` / `{body}` | Commit title / body |
| `{commit}` | Abbreviated commit ID |

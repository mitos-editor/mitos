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

`s` means status, `S` stashes, `f` file history, and `l` log.

Find other Git commands by searching for `vcs-` in the command palette (`Space ?`).

Existing commands such as `:diff`, `:file-history`, and `:git-history` still work.
`Space i` also inspects a change, and `Space B` toggles inline blame.

Previously, `Space g` opened changed files directly; that action is now
`Space g s`. To open changed files directly again, replace the
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

Conflicts with marker lines appear as labeled sections in the preview, including
unsaved edits. Press `Enter` to open the file and
[resolve the conflicts](#resolving-conflicts). Once the markers are removed, the
preview shows a diff so you can review the result.

| Key | Action |
| --- | --- |
| `Enter` | Open the file |
| `Alt-o` | Open the selected diff in a review buffer |
| `Alt-k` / `Alt-j` or `Alt-Up` / `Alt-Down` | Scroll the preview |
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

For conflicted files, use `:diff` to compare with `HEAD`. Save and stage the
resolved file before choosing another comparison.

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
method, and whether the review is loading.

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
keep the chosen commits and the buffer text shown when you opened them, even
after a refresh.

To review two files side by side:

1. Press `Space g s`, select a file, and press `Alt-o`.
2. Press `Space '` to restore the picker and select another file.
3. Press `Ctrl-v` to open the second diff in a vertical split.

Use `Ctrl-w v` / `Ctrl-w s` to split an existing review. The command palette also
lists the diff commands.

Binary files and submodules show an explanation instead of a text diff. Very
large files may be rejected, or their diffs shortened with a notice.

### Inspecting the current change

Press `Space i` or run `:show-change` to inspect the change at the cursor in a
popup. It compares `HEAD` with the current buffer, including unsaved edits, and
shows added and removed lines with context and line numbers.

Use `Ctrl-d` / `Ctrl-u` or Page Down / Page Up to scroll, Left / Right to scroll
horizontally, and `Esc` to close.

Press `r` to restore the inspected change from `HEAD`. The popup closes; press
`u` in the buffer to undo the restoration. Only the change at the inspected
cursor is restored, even if nearby changes appear in the popup. Restoration does
not save the file or change Git's staging area. If the buffer, cursor selection,
or repository changes while the popup is open, reopen it before restoring.

Remap scrolling and closing under `[keys.popup]`, and horizontal scrolling and
restoration under `[keys.diff]`. For example:

```toml
[keys.diff]
r = "unbound"
A-r = "restore"
```

The popup shows the configured restoration shortcut below the change.

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
| `:restore-change` | Restore each hunk touched by the selections from `HEAD` |

Restoring replaces each selected hunk in the current buffer. Use `u` to
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

The picker shows these shortcuts below the file list, including any remappings.
Staging and unstaging keep the query and selected file.

Staging does not save unsaved buffer edits. Staging and unstaging affect the
entire file, including both paths of a rename.

Stashing saves staged, unstaged, and untracked files throughout the repository,
then restores its working tree to `HEAD`. Ignored files and changes inside
submodules are excluded. Save modified buffers in that repository first.

While stashing or applying a stash, file saves in that repository are temporarily
blocked. Mitos updates open buffers afterwards. Edits you make during the
operation are kept; saving them may require `:w!` because Git changed the file
on disk.

### Staging individual changes

Use these commands in a file buffer, a live review, or a
[project review](#project-review) to add selected changes to Git's staging area
or remove them. Neither action changes the saved file or your unsaved edits.

| Command | Action |
| --- | --- |
| `:vcs-stage-hunk` | Stage each hunk touched by the selections |
| `:vcs-stage-lines` | Stage only selected lines |
| `:vcs-unstage-hunk` | Unstage each hunk touched by the selections |
| `:vcs-unstage-lines` | Unstage only selected lines |
| `:vcs-stage-and-next` | Stage the selected hunks and select the next unstaged hunk |

Stage commands use the current buffer, including unsaved edits. You can stage an
edit without saving it. The `:vcs-stage-hunk-disk` and `:vcs-stage-lines-disk`
commands use the saved file instead; save your buffer before running them.

Unstage commands leave selected changes out of the next commit without changing
the saved file or your unsaved edits. The gutter and `:show-change` show all
changes since `HEAD`, so one displayed hunk can include both staged and unstaged
changes.

For precise line choices, open `:diff buffer` to include unsaved edits, or
`:diff unstaged` to review the saved file. Select added or removed lines and run
`:vcs-stage-lines`; additions and removals can be staged separately. To leave
selected staged changes out of the next commit, open `:diff staged` and use
`:vcs-unstage-lines`. Reviews of past commits cannot stage or unstage changes.
If the file, staging area, or current commit changes after you open a review, run
`:diff-refresh` before trying again.

Line and hunk actions require UTF-8 text and support normal line-ending
conversions. Use whole-file actions in the changed-file picker for binary files,
other encodings, submodules, and symlinks. Also stage the whole file if Git's file
conversion changes its line count, or when changing the file's encoding. Stage a
rename as a whole file before staging individual text changes.

The current-change popup (`Space i`) also offers `s` to stage the cursor's hunk,
`u` to unstage it, and `S` to stage and select the next unstaged hunk. These actions
add changes to or remove them from the staging area. The preview shows all
changes since `HEAD`, including staged and unstaged changes. Actions close the
popup; press `Space i` again to review the result. Remap them under `[keys.diff]`
using `stage`, `unstage`, and `stage_and_next`.

### Stashes

Run `:vcs-stashes`, press `Space g S`, or choose `git_stash_picker` from the
command palette to browse the newest 500 stashes. Press `Enter` to browse a
stash's saved staged version, working tree, and untracked files, then `Enter` on
a file to review its saved changes. `Alt-Enter` keeps the picker open.

In the stash picker, use these shortcuts:

| Key | Action |
| --- | --- |
| `Alt-s` | Apply the stash and keep it |
| `Alt-i` | Apply the stash, restore its staged changes, and keep it |

Applying a stash asks for confirmation. Save modified buffers in that repository
first. Git may leave conflicts to resolve; the stash is kept even if applying it
fails. Mitos refreshes the resulting files and staged changes.

Remap Git and stash actions under `[keys.git-picker]` and `[keys.stash-picker]`.
See [Component keys](./remapping.md#component-keys).

## Committing staged changes

Stage the changes you want to commit, then:

1. Run `:vcs-commit` to open your staged changes beside a commit message.
2. Review the changes and write your message using normal editing commands.
3. Run `:vcs-commit-submit` from the message buffer to commit.

The message buffer's name shows which repository you are committing to.
Committing uses the reviewed staged changes and your Git settings for author,
hooks, and signing. Unsaved edits in other buffers stay unsaved. A commit is
stored locally; push it when you are ready to share it.

If the staged changes or current commit change while you write, run `:vcs-commit`
again. This updates the review and keeps your message. If Git rejects the commit,
the message stays open so you can fix the problem and retry.

If another Git command changes the staging area during a commit, Mitos keeps
those changes and asks you to review them before committing again.

Mitos requires a complete staged review before committing. If a file cannot be
shown or the review is too large, read the notice in the review and use Git in
your terminal if needed.

## Branches and worktrees

Git commands use the current file's repository, including when it differs from
the current working directory. With no source file open, they use the current
directory. Repository changes require [workspace trust](./workspace-trust.md).

| Command | Action |
| --- | --- |
| `:vcs-branches` | Browse local branches; `Enter` switches to the selected branch |
| `:vcs-branch-create <name>` | Create a branch at `HEAD` without switching |
| `:vcs-branch-switch <name>` | Switch to an existing local branch |
| `:vcs-worktrees` | Browse worktrees; `Enter` opens the selected worktree |
| `:vcs-worktree-create <path> [branch]` | Create a linked worktree at a new path |
| `:vcs-worktree-open <path>` | Change directory and open that worktree's file picker |
| `:vcs-worktree-remove <path>` | Confirm removal of a clean linked worktree |

Save modified buffers before switching branches. Git keeps changes on disk and
refuses a switch that would overwrite them. Branch switching does not create
remote-tracking branches automatically.

Create linked worktrees outside the current working tree. Without a branch name,
a new worktree uses detached `HEAD`. Supply an existing local branch to check it
out instead; Git refuses a branch already checked out in another worktree.

Opening a worktree keeps existing buffers and their unsaved edits. Each buffer
continues to belong to its original repository. Choose a file from the new
worktree to work in that checkout.

Removing a worktree asks for confirmation and keeps the branch. Main, current,
and locked worktrees cannot be removed. Modified, untracked, and ignored files
also prevent removal. Change directory out of a worktree before removing it.

Reopen a picker to refresh the branch or worktree list after external changes.

## Fetch, pull, and push

Remote operations run only when you request them and require workspace trust.
Use `:vcs-remotes` to choose a remote by name, then choose an operation. You can
also run these commands directly:

| Command | Action |
| --- | --- |
| `:vcs-fetch [remote]` | Download remote commits without changing working files |
| `:vcs-pull [remote]` | Update the current branch by fast-forward only |
| `:vcs-push [remote]` | Upload the current branch without forcing |

When exactly one remote is configured, its name is optional. With several
remotes, choose one explicitly. Remote URLs are not shown in the picker.

Pulling requires saved buffers and does not merge, rebase, or stash changes.
Diverged branches must be reconciled explicitly in Git. Pushing uploads only the
current local branch, without mirroring or automatically pushing tags. When the
selected remote matches the branch's configured upstream, its upstream branch
name is used; otherwise the current branch name is used. Detached `HEAD` must be
put on a branch before pulling or pushing.

Git uses your configured credentials and SSH agent. Terminal password prompts
are disabled while commands run in Mitos. For detailed network errors, run the
corresponding Git command in your terminal.

## Git history

Use history to review older changes or compare a past version with your current
buffer. Browsing history does not check out revisions or change files or staged
changes. Choose `git_file_history` or `git_history` from the command palette
(`Space ?`), or use the commands below.

### File history

Run `:file-history` to browse commits that changed the current file, following
renames detected by Git. The preview shows the selected commit's diff against
its first parent. `Ctrl-t` hides or shows the preview; `Alt-k` / `Alt-j` or
`Alt-Up` / `Alt-Down` scroll it.

Press `Enter` to open the diff in a review buffer. `Ctrl-v` / `Ctrl-s` open it in a
vertical / horizontal split. `Space '` restores the picker with its query and
selection, so you can open another comparison alongside it.

Use these shortcuts directly on the selected commit:

| Key | Action |
| --- | --- |
| `Alt-v` | Open the file's historical version |
| `Alt-e` | Compare the revision with the current buffer, including unsaved edits |
| `Alt-p` | Choose a parent to compare against |

Comparing with the buffer requires an editable buffer for that file.

File history follows the first parent of a merge: the previous commit on the
branch that received the merge. Use repository history to see individual commits
from merged branches.

### Repository history

Run `:git-history` (alias `:vcs-history`) to browse commits leading to `HEAD`,
including commits from merged branches. It uses the current file's repository,
or the current directory when no file is open. Run the command again after
switching branches to browse the new branch.

Select a commit, then a changed file to preview and review its diff. Merges
compare with their first parent by default; use `Alt-p` to choose another parent.
In the changed-file browser, `Alt-v` opens the historical version and `Alt-e`
compares it with an open editable buffer for that file, including unsaved edits.

In the changed-file browser, `Alt-h` returns to the commit browser with its query
and selection. `Space '` restores the last picker after opening a review.

### Older versions and comparisons

Historical versions open in read-only buffers with syntax highlighting. In a
historical diff, `Enter` on a removed line opens the original version; on an added
or unchanged line it opens the updated version. When comparing with unsaved
edits, it opens the text shown when you started the review. Use `:diff-open-file`
to open the current file instead.

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

## Resolving conflicts

Press `Space g s` and open a conflicted file. Use `]m` to move to the next
conflict or `[m` to move to the previous one.

Place the cursor inside a conflict and press `Space a`. Choose **Accept current**,
**Accept incoming**, or **Accept both (current then incoming)** to keep that text.
The menu also includes other available code actions. You can edit the text directly.

Press `u` to undo a choice. If you keep both sides, review and edit the combined
text. When you have resolved every conflict, save the file and stage it.

The editor labels each side of the conflict. To see or edit the marker lines,
run `:vcs-conflict-toggle-markers`. Run it again to restore the headings. This
affects only the current view.

You can also resolve conflicts with commands:

| Command | Action |
| --- | --- |
| `:vcs-conflict-resolve current` | Keep the current side |
| `:vcs-conflict-resolve incoming` | Keep the incoming side |
| `:vcs-conflict-resolve both` | Keep current followed by incoming |

During a merge, **current** is the branch you have checked out and **incoming** is
the branch you are merging. During a rebase, **current** is the branch you are
rebasing onto, including commits already applied; **incoming** is the commit being
applied.

To see the whole file for either side, run `:vcs-conflict-source current` or
`:vcs-conflict-source incoming`. Use `:vcs-conflict-source base` to see the original
version. Each version opens in a read-only split. Close it with `:q`.

If Mitos cannot recognize a conflict, edit the file and remove the marker lines
yourself.

## Project review

Run `:vcs-project-diff` to review unstaged changes across the repository in one
read-only buffer, including unsaved edits. Choose `:vcs-project-diff staged` to
review what the next commit will contain, or `:vcs-project-diff unstaged` to
compare the staging area with saved files.

Each file has a heading followed by its changes, with added and removed lines
marked. Move, select, search, and yank as usual. Binary files and other files
that cannot be reviewed show an explanation; you can keep reviewing the rest.

| Command | Action |
| --- | --- |
| `:vcs-project-next file` / `previous-file` | Move between files |
| `:vcs-project-next hunk` / `previous-hunk` | Move between hunks |
| `Enter`, `:vcs-project-open` | Open the selected file's review |
| `:vcs-project-refresh` | Update the review with the latest changes |

Use `[g` / `]g` to move between hunks across files, and `[G` / `]G` for the
first / last hunk. Counts such as `2]g` work as they do in file reviews.
Press `Enter` to inspect one file's review, then `Ctrl-o` to return to the
project review.

Run `:vcs-stage-hunk` or `:vcs-stage-lines` directly in a project review to stage
the selected change. Use a staged project review with `:vcs-unstage-hunk` or
`:vcs-unstage-lines` to leave selected changes out of the next commit. You can
select additions and removals separately. Keep each action's selections within
one file; selections spanning files are rejected. These actions keep your saved
files and unsaved edits.

When staging from the default review, unsaved edits shown for that file are
included without saving it. An unstaged review stages the saved content instead.
If you edit a file or change the staging area after opening the review, run
`:vcs-project-refresh` before staging or unstaging.

Use `:vcs-stage-and-next` to stage the selected hunks and move to the next
remaining hunk in that file, then to the next changed file.

Git actions in Mitos update open project reviews automatically. After editing or
running Git outside Mitos, use `:vcs-project-refresh` to see the latest changes.
If files change while the review is loading, refresh to try again.

Very large reviews may be shortened, with a notice explaining what was left out.
Use the changed-file picker to review the remaining files individually.

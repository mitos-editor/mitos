# Review and stage changes

After [finding and editing code](./project-navigation.md), review what changed
before preparing a commit. Mitos can show unsaved edits, files on disk, and
Git's staging area. Those versions can differ.

This walkthrough uses Git. Start in a repository with a tracked file that has
no uncommitted changes, and make a small edit using the techniques from the
earlier articles. For example, change a word in a comment with `miw`, `c`, the
new text, and `Esc`. Leave the edit unsaved for the first step.

## Review your edit before saving

1. Press `Space g d` to compare the current buffer with `HEAD`, your
   checked-out commit. The review includes the unsaved edit.
2. Use `]g` and `[g` to move between groups of changed lines, called hunks.
   Removed and added lines show the old and new text.
3. Press `Enter` on an added or unchanged line to return to that location in
   the source file. You can also use `Ctrl-o` to return through the jumplist.

The review is read-only. Opening it does not save the source file or stage
anything. If you see an unintended change, return to the source and fix it,
then review again. Current-file reviews update as the source changes.

For a quick look at one change, put the cursor on it in the source file and
press `Space g i`. Press `Esc` to close the popup.

## Save, then stage the intended version

Staging chooses what goes into the next commit. It uses the file on disk,
so save the version you want to include first.

1. In the source buffer, run `:write`.
2. After the save completes, press `Space g s` to open the changed-file picker
   and select that file.
3. Press `Alt-s` to stage it. Staging needs Git on your `PATH` and
   [workspace trust](../workspace-trust.md); use `:workspace-trust` for a project
   you trust if Mitos asks for it.
4. Look at the staged and unstaged columns. For this exercise, the change
   should now be staged, with no remaining unstaged change in that file.
5. Use `Alt-p` or `Alt-n` until the preview shows the **staged** comparison.
   This compares `HEAD` with the version selected for the next commit.
6. Press `Alt-o` to open that comparison in a review buffer.

`Alt-s` stages the entire file, not just the highlighted lines or current hunk.
It does not save unsaved buffer edits. If a file contains changes that belong
in separate commits, save it and use `git add -p` in your terminal to stage
selected hunks.

To undo a staging decision, reopen `Space g s`, select the file, and press
`Alt-u`. The saved file stays changed; only the staged version is reset.
Stage it again and inspect the staged comparison before committing.

## Understand the three versions

| Version | How it changes | What to review |
| --- | --- | --- |
| Buffer | You edit text in Mitos | `Space g d` compares `HEAD` with the buffer, including unsaved edits |
| File on disk | You run `:write` | `:vcs-diff unstaged` compares the staged version with the file on disk |
| Staging area | You stage the saved file | `:vcs-diff staged` compares `HEAD` with the staged version |

If you edit again after staging, your next commit still contains the previously
staged text. Save and stage again to include the new edit, or leave it unstaged
for a later commit. Use `:vcs-diff buffer` to compare the staging area directly
with your current buffer.

When the staged diff contains what you intend, make the commit in your terminal:

```sh
git diff --cached
git commit -m "Describe the change"
```

The commit contains staged changes throughout the repository, including files
you staged earlier. Review that full diff even if you edited only one file in
this exercise.

## Find out why a line changed

History helps when an unfamiliar piece of code has a reason behind it:

1. Put the cursor on an unchanged line and press `Space g b`. The blame popup
   identifies the commit that last changed it and shows its message.
2. Press `Esc`, then `Space g f` to browse commits that changed this file.
3. Read the preview, or press `Enter` to open a commit's diff.
4. Use `Ctrl-o` to return through your earlier locations.

Browsing history opens read-only versions; it does not switch your branch or
replace working files. A line with uncommitted changes can show **Not committed
yet** rather than a previous commit. Use `Space g l` when you need the history
of the whole repository rather than one file.

## Restore one experimental change

In the source buffer, place the cursor on the unwanted change and press
`Space g i` to inspect it. Press `r` in that popup to restore the inspected
hunk from `HEAD`, then check the result. Press `u` in the source buffer to undo
the restoration.

Restoration changes only the buffer. It neither saves the file nor updates the
staging area. If you keep the restored text, save it and review the versions
again before staging.

For the full command list and workflows for conflicts, stashes, branches, and
worktrees, see [Version control](../version-control.md).

**Previous:** [Navigate a project](./project-navigation.md) ·
**Back to:** [Editing effectively](../editing-effectively.md)

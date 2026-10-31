| Name | Description |
| --- | --- |
| `:exit`, `:x`, `:xit` | Write changes to disk if the buffer is modified and then quit. Accepts an optional path (`:exit some/path.txt`). |
| `:exit!`, `:x!`, `:xit!` | Force write changes to disk, creating necessary subdirectories, if the buffer is modified and then quit. Accepts an optional path (`:exit! some/path.txt`). |
| `:quit`, `:q` | Close the current view. |
| `:quit!`, `:q!` | Force close the current view, ignoring unsaved changes. |
| `:open`, `:o`, `:edit`, `:e` | Open a file from disk into the current view. |
| `:buffer-close`, `:bc`, `:bclose` | Close the current buffer. |
| `:buffer-close!`, `:bc!`, `:bclose!` | Close the current buffer forcefully, ignoring unsaved changes. |
| `:buffer-close-others`, `:bco`, `:bcloseother` | Close all buffers but the currently focused one. |
| `:buffer-close-others!`, `:bco!`, `:bcloseother!` | Force close all buffers but the currently focused one. |
| `:buffer-close-all`, `:bca`, `:bcloseall` | Close all buffers without quitting. |
| `:buffer-close-all!`, `:bca!`, `:bcloseall!` | Force close all buffers ignoring unsaved changes without quitting. |
| `:buffer-next`, `:bn`, `:bnext` | Goto next buffer. |
| `:buffer-previous`, `:bp`, `:bprev` | Goto previous buffer. |
| `:write`, `:w` | Write changes to disk. Accepts an optional path (`:write some/path.txt`). |
| `:write!`, `:w!` | Force write changes to disk creating necessary subdirectories. Accepts an optional path (`:write! some/path.txt`). |
| `:write-buffer-close`, `:wbc` | Write changes to disk and closes the buffer. Accepts an optional path (`:write-buffer-close some/path.txt`). |
| `:write-buffer-close!`, `:wbc!` | Force write changes to disk creating necessary subdirectories and closes the buffer. Accepts an optional path (`:write-buffer-close! some/path.txt`). |
| `:new`, `:n` | Create a new scratch buffer. |
| `:format`, `:fmt` | Format the file using an external formatter or language server. |
| `:indent-style` | Set the indentation style for editing. ('t' for tabs or 1-16 for number of spaces.) |
| `:line-ending` | Set the document's default line ending. Options: crlf, lf. |
| `:earlier`, `:ear` | Jump back to an earlier point in edit history. Accepts a number of steps or a time span. |
| `:later`, `:lat` | Jump to a later point in edit history. Accepts a number of steps or a time span. |
| `:write-quit`, `:wq` | Write changes to disk and close the current view. Accepts an optional path (`:wq some/path.txt`). |
| `:write-quit!`, `:wq!` | Write changes to disk and close the current view forcefully. Accepts an optional path (`:wq! some/path.txt`). |
| `:write-all`, `:wa` | Write changes from all buffers to disk. |
| `:write-all!`, `:wa!` | Forcefully write changes from all buffers to disk creating necessary subdirectories. |
| `:write-quit-all`, `:wqa`, `:xa` | Write changes from all buffers to disk and close all views. |
| `:write-quit-all!`, `:wqa!`, `:xa!` | Forcefully write changes from all buffers to disk, creating necessary subdirectories, and close all views after successful saves. |
| `:quit-all`, `:qa` | Close all views. |
| `:quit-all!`, `:qa!` | Force close all views ignoring unsaved changes. |
| `:cquit`, `:cq` | Quit with exit code (default 1). Accepts an optional integer exit code (`:cq 2`). |
| `:cquit!`, `:cq!` | Force quit with exit code (default 1) ignoring unsaved changes. Accepts an optional integer exit code (`:cq! 2`). |
| `:theme` | Change the editor theme (show current theme if no name specified). |
| `:yank-join` | Yank joined selections. A separator can be provided as first argument. Default value is newline. |
| `:clipboard-yank` | Yank main selection into system clipboard. |
| `:clipboard-yank-join` | Yank joined selections into system clipboard. A separator can be provided as first argument. Default value is newline. |
| `:primary-clipboard-yank` | Yank main selection into system primary clipboard. |
| `:primary-clipboard-yank-join` | Yank joined selections into system primary clipboard. A separator can be provided as first argument. Default value is newline. |
| `:clipboard-paste-after` | Paste system clipboard after selections. |
| `:clipboard-paste-before` | Paste system clipboard before selections. |
| `:clipboard-paste-replace` | Replace selections with content of system clipboard. |
| `:primary-clipboard-paste-after` | Paste primary clipboard after selections. |
| `:primary-clipboard-paste-before` | Paste primary clipboard before selections. |
| `:primary-clipboard-paste-replace` | Replace selections with content of system primary clipboard. |
| `:show-clipboard-provider` | Show clipboard provider name in status bar. |
| `:change-current-directory`, `:cd` | Change the current working directory. |
| `:show-directory-stack` | Show the directory stack as a space-delimited string. |
| `:push-directory`, `:pushd` | Save and then change the current directory. |
| `:pop-directory`, `:popd` | Remove the top entry from the directory stack, and cd to the new top directory.. |
| `:show-directory`, `:pwd` | Show the current working directory. |
| `:encoding` | Set encoding. Based on `https://encoding.spec.whatwg.org`. |
| `:character-info`, `:char` | Get info about the character under the primary cursor. |
| `:reload`, `:rl` | Discard changes and reload from the source file. |
| `:reload-all`, `:rla` | Discard changes and reload all documents from the source files. |
| `:update`, `:u` | Write changes only if the file has been modified. |
| `:lsp-workspace-command` | Open workspace command picker |
| `:lsp-restart` | Restarts the given language servers, or all language servers that are used by the current file if no arguments are supplied |
| `:lsp-stop` | Stops the given language servers, or all language servers that are used by the current file if no arguments are supplied |
| `:tree-sitter-scopes` | Display tree sitter scopes, primarily for theming and development. |
| `:tree-sitter-highlight-name` | Display name of tree-sitter highlight scope under the cursor. |
| `:tree-sitter-layers` | Display language names of tree-sitter injection layers under the cursor. |
| `:debug-start`, `:dbg` | Start a debug session from a given template with given parameters. |
| `:debug-remote`, `:dbg-tcp` | Connect to a debug adapter by TCP address and start a debugging session from a given template with given parameters. |
| `:debug-eval` | Evaluate expression in current debug context. |
| `:vsplit`, `:vs` | Open the file in a vertical split. |
| `:vsplit-new`, `:vnew` | Open a scratch buffer in a vertical split. |
| `:hsplit`, `:hs`, `:sp` | Open the file in a horizontal split. |
| `:hsplit-new`, `:hnew` | Open a scratch buffer in a horizontal split. |
| `:tutor` | Open the tutorial. |
| `:goto`, `:g` | Goto line number. |
| `:set-language`, `:lang` | Set the language of current buffer (show current language if no value specified). |
| `:set-spelling-language`, `:spelling` | Set the spell-checking languages for the current buffer (e.g. `en_US`); a word is flagged only when every language rejects it. Pass `off` to disable, or no value to show the current languages. |
| `:set-option`, `:set` | Set a config option at runtime.<br>For example to disable smart case search, use `:set search.smart-case false`. |
| `:toggle-option`, `:toggle` | Toggle a config option at runtime.<br>For example to toggle smart case search, use `:toggle search.smart-case`. |
| `:get-option`, `:get` | Get the current value of a config option. |
| `:sort` | Stably sort selected contents. Supports --numeric (-n), --natural (-N), --length (-l), --key (-k) REGEX and --ignore-leading-blanks (-b), plus --insensitive (-i) and --reverse (-r). |
| `:reflow` | Hard-wrap the current selection of lines to a given width. |
| `:tree-sitter-subtree`, `:ts-subtree` | Display the smallest tree-sitter subtree that spans the primary selection, primarily for debugging queries. |
| `:snippet-insert` | Insert a named user-defined snippet at the selections. |
| `:snippets-reload` | Reload global and trusted workspace snippet definitions. |
| `:snippets-open` | Open the user snippet configuration or a named .toml file in snippets/. |
| `:snippets-open-workspace` | Open the workspace snippet configuration or a named .toml file in .mitos/snippets/. |
| `:config-reload` | Refresh user config. |
| `:config-open` | Open the user config.toml file. |
| `:config-migrate` | Migrate legacy user settings after confirmation, creating a backup. |
| `:config-migrate-workspace` | Migrate legacy trusted workspace settings after confirmation, creating a backup. |
| `:config-open-workspace` | Open the workspace config.toml file. |
| `:log-open` | Open the mitos log file. |
| `:insert-output` | Run shell command, inserting output before each selection. |
| `:append-output` | Run shell command, appending output after each selection. |
| `:pipe`, `:\|` | Pipe each selection to the shell command. |
| `:pipe-to` | Pipe each selection to the shell command, ignoring output. |
| `:run-shell-command`, `:sh`, `:!` | Run a shell command |
| `:vcs-conflict-next` | Move to the next conflict; pass previous to move backwards. |
| `:vcs-conflict-resolve` | Keep the current side, incoming side, or both sides of the conflict at the cursor. |
| `:vcs-conflict-toggle-markers` | Show or hide raw conflict markers in the current view. |
| `:vcs-conflict-source` | Open the base, current, or incoming version of a conflicted file in a read-only split. |
| `:vcs-project-diff` | Review unstaged changes together, including unsaved edits. Use staged for the next commit or unstaged for saved changes. |
| `:vcs-project-refresh` | Update the project review with the latest file and staged changes. |
| `:vcs-project-next` | Move to the next file or hunk; use file, previous-file, hunk, or previous-hunk. |
| `:vcs-project-open` | Open the selected file's review. Use Ctrl-o to return to the project review. |
| `:blame`, `:vcs-blame` | Show commit details for the current line, with actions to copy the commit ID or message. |
| `:vcs-commit`, `:git-commit` | Review staged changes and write a commit message. Run again to update the review and keep the message. |
| `:vcs-commit-submit`, `:git-commit-submit` | Commit the reviewed changes with this message. If Git rejects the commit, the message stays open. |
| `:vcs-branches`, `:git-branches` | Browse local branches. Enter switches to the selected branch without discarding changes. |
| `:vcs-branch-create`, `:git-branch-create` | Create a local branch at HEAD without switching to it. |
| `:vcs-branch-switch`, `:git-branch-switch` | Switch to an existing local branch. Save edited files first; Git refuses to overwrite changes. |
| `:vcs-worktrees`, `:git-worktrees` | Browse worktrees. Enter opens the selected worktree's file picker. |
| `:vcs-worktree-create`, `:git-worktree-create` | Create a worktree in a new folder. Give an existing branch name, or open the current commit without a branch. |
| `:vcs-worktree-open`, `:git-worktree-open` | Change directory to a worktree and open its file picker, keeping your open files. |
| `:vcs-worktree-remove`, `:git-worktree-remove` | Confirm removal of a clean worktree, keeping its branch. |
| `:vcs-remotes`, `:git-remotes` | Choose a remote, then fetch, pull, or push. |
| `:vcs-fetch`, `:git-fetch` | Download remote commits without changing your files. Give a remote name when several are configured. |
| `:vcs-pull`, `:git-pull` | Download changes and move the current branch forward without creating a merge commit. Optionally give a remote name. |
| `:vcs-push`, `:git-push` | Upload commits from the current branch. Optionally give a remote name. |
| `:show-change`, `:vcs-show-change` | Inspect the current change against HEAD, including deleted lines and unsaved edits. |
| `:diff`, `:vcs-diff` | Review the current file. Choose head (all changes), staged (next commit), unstaged (saved changes), or buffer (unstaged and unsaved changes). |
| `:git-history`, `:vcs-history` | Browse repository commits. Select a commit, then a file to review its changes. |
| `:file-history`, `:git-file-history`, `:vcs-file-history` | Browse commits that changed the current file, following renames. Enter reviews the selected change; Alt-v opens its version, Alt-e compares the buffer, and Alt-p chooses a parent. |
| `:diff-refresh`, `:vcs-diff-refresh` | Reload the current diff. Reviews of past commits keep the versions originally chosen. |
| `:diff-open-source`, `:vcs-diff-open-source` | Open the source at the selected diff line. |
| `:diff-open-file`, `:vcs-diff-open-file` | Open the current file from a diff review. |
| `:vcs-stage-hunk` | Stage hunks touched by the selections, including unsaved edits. Does not save the file. |
| `:vcs-stage-lines` | Stage only selected lines, including unsaved edits. Does not save the file. |
| `:vcs-stage-hunk-disk` | Stage hunks touched by the selections from the saved file. Save the buffer first. |
| `:vcs-stage-lines-disk` | Stage only selected lines from the saved file. Save the buffer first. |
| `:vcs-unstage-hunk` | Unstage hunks touched by the selections, keeping saved and unsaved edits. |
| `:vcs-unstage-lines` | Unstage only selected lines, keeping saved and unsaved edits. |
| `:vcs-stage-and-next` | Stage the selected hunks and move to the next unstaged hunk. |
| `:restore-change`, `:vcs-restore-change` | Restore selected hunks from HEAD in the buffer. Use u to undo; saved files and staged changes stay unchanged. |
| `:vcs-changes`, `:git-changes` | Browse changed files in the workspace. |
| `:vcs-repository-changes`, `:git-repository-changes` | Browse changed files throughout the current repository. |
| `:vcs-stashes`, `:git-stashes` | Browse Git stashes in the current repository. |
| `:vcs-toggle-inline-blame`, `:git-toggle-inline-blame` | Show or hide inline Git blame. |
| `:clear-register` | Clear given register. If no argument is provided, clear all registers. |
| `:set-register` | Set contents of the given register. |
| `:redraw` | Clear and re-render the whole UI |
| `:move`, `:mv` | Move the current buffer and its corresponding file to a different path |
| `:move!`, `:mv!` | Move the current buffer and its corresponding file to a different path creating necessary subdirectories |
| `:yank-diagnostic` | Yank diagnostic(s) under primary cursor to register, or clipboard by default |
| `:read`, `:r` | Load a file into buffer |
| `:echo` | Prints the given arguments to the statusline. |
| `:noop` | Does nothing. |
| `:workspace-trust` | Allow language servers and local config for the current workspace. |
| `:workspace-untrust` | Revoke the current workspace's trust grant or exclusion. |
| `:workspace-exclude` | Mark the current workspace as never-prompt. Never prompts for trust again. |

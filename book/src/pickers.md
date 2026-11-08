# Pickers

Pickers let you find files, open buffers, search results, symbols, and diagnostics.
Type to filter, move through the results, then press `Enter` to open one.
The title in the top border identifies the picker.

| Keys | Picker |
| --- | --- |
| `Space f` / `Space F` | Files at the workspace root / current working directory |
| `Space b` | Open buffers |
| `Space e` / `Space E` | File explorer at the workspace / current file's directory |
| `Space /` | Project search |
| `Space s` / `Space S` | File / workspace symbols |
| `Space d` / `Space D` | File / workspace diagnostics |
| `Space g s` | [Changed Git files](./version-control.md#changed-files) |
| `Space '` | Reopen the last picker |

## Open a result

Use `Ctrl-n` and `Ctrl-p` or the arrow keys to move between results. `Enter`
opens the selected item; `Alt-Enter` opens it in the background while keeping
the picker open. `Ctrl-v` opens a vertical split and `Ctrl-s` a horizontal one.
`Ctrl-t` toggles the preview. `Esc` closes the picker.

Scroll the preview with `Alt-j` / `Alt-k` or `Alt-Down` / `Alt-Up`. Each press
moves half a page without changing the selected result or query. These keys
work in file, buffer, search, symbol, diagnostic, directory, and Git previews.
You can also use the mouse wheel over the preview. A scrollbar shows your
position when the preview extends beyond the pane.
Selecting a different file or match resets the preview to that location.
Images fit the preview pane and do not scroll.

Remap `preview_scroll_down` and `preview_scroll_up` under `[keys.picker]`, or
under a specific picker's component keymap.

Press `Ctrl-q` to copy matched file locations into the [quicklist](./quicklist.md)
for navigation after closing the picker. To customize keys, see
[Component keys](./remapping.md#component-keys).

## Layout

By default, pickers fill the available area on terminals narrower than 200
columns and use a centered layout on wider terminals. To keep margins around
pickers at every terminal size, set:

```toml
[editor.picker]
layout = "centered"
```

The centered layout uses 90% of the available width and height. Both layouts
reserve the bottom two rows. This preference applies to all pickers, including
the command palette. Use `layout = "auto"` to restore the default behavior.

## Filtering results

Most pickers use fuzzy matching with [fzf search syntax](https://github.com/junegunn/fzf#search-syntax).
For example, `main rs` matches both terms, `'main` requires an exact substring,
and `!test` excludes matches. OR operations (`|`) are not supported.

Global search uses regular expressions for the text query. Workspace symbol
queries go to the language server when one supplies workspace symbols; syntax
symbol search is available as a fallback. See [Language servers](./lsp.md#symbols-without-a-server).

In pickers with several columns, prefix a column filter with `%`. Column names
can be shortened: `%p`, `%pa`, and `%path` all select the path column. For
example, `mitos %p .toml !lang` in global search looks for `mitos`, filtering
paths by `.toml` and excluding paths matching `lang`. Use `.toml$` to require
the path to end in `.toml`.

Insert a register into a query with `Ctrl-r`, then its name. `Ctrl-r .`
inserts the current selections; `Ctrl-r %` inserts the current file's path.
See [Search and replacement](./search.md) for project-search examples.

## Replacing global search results

Global search uses smart case by default. Press `Alt-c` to toggle between
smart case and case-sensitive matching. The input bar shows the current mode.

Press `Alt-r` to reveal the replacement input. Press it again to switch focus
between the search and replacement inputs. While the replacement input is
visible, `Enter` replaces the selected match; `Ctrl-Enter`, `Cmd-Enter`, or
`Win-Enter` replaces every occurrence in the files matching the current path
filter. Wait for search and result updates to finish before replacing.

Replacement text expands regex captures: `$1` and `${name}` insert numbered
and named captures, and `$$` inserts a literal dollar sign. An empty replacement
deletes matches. Open buffers, including unsaved edits, take precedence over files
on disk. Their replacements stay unsaved and form one undo step per buffer.
Files without an open buffer are updated directly on disk, without adding them
to the buffer list or editor undo history. An error stops further writes;
completed writes remain applied. Closing the picker does not interrupt file
writes that have already started.

Results and previews refresh after replacement so subsequent actions use current
match positions. These bindings can be changed under `[keys.picker]`.

## File explorer

Press `Space e` to browse the workspace, or `Space E` for the current file's
directory. Select a directory and press `Enter` to enter it. Select `../` to
go to its parent. The query filters entries in the directory being displayed.

Supported [images](./image-previews.md) also appear in the preview pane.

Manage files without leaving the explorer:

| Key | Action |
| --- | --- |
| `Alt-n` | Create a file in the shown directory. End the path with `/` to create a directory. |
| `Alt-m` | Move or rename the selected entry. The cursor starts before the file extension. Open buffers follow the move. |
| `Alt-c` | Copy the selected file's saved contents. Copying directories is not supported. |
| `Alt-d` | Delete the selected entry after you type `y` and press `Enter`. Directories are deleted with their contents. |
| `Alt-y` | Yank the selected path to the default yank register. |

Create, move, and copy open a path prompt. Edit the path and press `Enter` to
apply the change, or press `Esc` to cancel. Paths are relative to the working
directory unless you enter an absolute path or start with `~`. The create prompt
starts with the directory being displayed.

Moving or copying onto an existing directory puts the entry inside it. Existing
paths are never overwritten, and missing parent directories are created. Remap
these keys under [`[keys.file-explorer]`](./remapping.md#component-keys).

### Sidebar

Press `Ctrl-e` in normal or select mode to open the file explorer sidebar at the
workspace root. Press it again to return to the editor. In the editor,
`Ctrl-w b` (or `Space w b`) shows or hides the sidebar, keeping expanded
directories and your selection.

| Key | Action |
| --- | --- |
| `j` / `k` or `Down` / `Up` | Select an entry |
| `Enter` | Expand or collapse a directory, or open a file |
| `/` | Start a search |
| `Enter` while searching | Confirm the search and navigate the filtered tree |
| `Esc` with a search | Clear the search and keep the sidebar focused |
| `Esc` without a search | Return to the editor |
| `q` while navigating | Hide the sidebar |

Search uses fuzzy matching and includes collapsed directories, excluding Git
and other version control metadata. Results show matching names with their
parent directories expanded, and hide unrelated files. Directories come first,
with names sorted alphabetically within each directory. Each new query selects
the first match; background updates keep your selection when possible.
Include a path separator to search relative paths. `Ctrl-w` deletes the previous
word in the search.
After confirming a search, press `Enter` again to open the selected file.
Opening a file returns focus to the editor. The sidebar keeps your search and
selection when you return. Press `/` to start a new search or `Esc` to clear it.

[File actions](#file-explorer), such as `Alt-n` to create a file and `Alt-m` to
move it, also work in the sidebar. Remap navigation keys under `keys.sidebar`
and search keys under `keys.sidebar-filter`; see [component keys](./remapping.md#component-keys).
For moving between the sidebar and editor splits, see [window navigation](./files.md#work-in-splits).

## Hidden and ignored files

The file picker and global search omit hidden and ignored files by default.
The file explorer shows most files, including hidden files, by default.
Configure them separately under [`editor.file-picker`](./configuration.md#editorfile-picker)
and [`editor.file-explorer`](./configuration.md#editorfile-explorer).

For example, show hidden files in the file picker:

```toml
[editor.file-picker]
hidden = false
```

Ignore rules can come from `.gitignore`, `.ignore`, parent directories, and
Git's global excludes file. The reference lists the switches for each source.
Git-specific rules apply only inside Git repositories.

To add Mitos-specific rules, use `.mitos/ignore` in a workspace or `ignore` in
your Mitos configuration directory. They use gitignore syntax, including `!`
to unignore a path:

```gitignore
!.github/
!.gitignore
!.gitattributes
```

## Loading large collections

Git history and branch pickers load more results as you scroll toward the end.
Typing a query filters loaded rows immediately and continues loading older or
remaining entries in the background. Clear the query to stop this extra loading;
an already requested page can still finish. Scrolling can then load more results.

Press `Ctrl-l` to pause loading. Press it again to continue, or to retry after a
loading failure. These controls keep your query and loaded results. Remap the
`toggle_loading` action under `[keys.picker]` or the specific picker table.
The active shortcut also appears along the bottom border when picker hints are
enabled.

Automatic loading pauses after an estimated 32 MiB of retained rows and matching
text. Continuing permits another 32 MiB for that picker session. This is a soft
budget: a page can cross it, and traversal state, previews, and other editor
memory are separate. Restoring a cached picker keeps its loaded results
and allowance; opening a new picker starts a fresh session.

The source status distinguishes more results available, loading, complete,
paused, and failed. A `?` in the result count means results are incomplete.
An empty result list can still gain matches while loading continues. Streaming
searches and symbol queries also remain incomplete if their source fails or
stops before reporting completion.

## Refreshing results

File, explorer, global-search, and changed-file results update as files change,
keeping your query, selected item, and scroll position where possible. You can
keep navigating while updates load. File previews prefer open buffers to files
on disk. Changed-file pickers show the
[selected comparison](./version-control.md#changed-files). Global search also
follows unsaved buffer edits.

Buffer and diagnostic pickers follow editor updates. A resolved diagnostic stays
visible while selected, marked as resolved, until you move away. Symbol pickers
refresh when their source documents or providers change. Reopening these pickers
checks their current sources again.

Quicklist and jump-history entries remain stored collections; filesystem changes
do not regenerate their contents. [Git history](./version-control.md#git-history)
starts from the checked-out commit when opened; run the history command again
to browse a different branch.

If results do not update, check [file watching](./configuration.md#editorfileswatch)
and [periodic checks](./configuration.md#editorfilesreload).

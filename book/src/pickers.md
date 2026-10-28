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
| `Space g c` | [Changed Git files](./version-control.md#changed-files) |
| `Space '` | Reopen the last picker |

## Open a result

Use `Ctrl-n` and `Ctrl-p` or the arrow keys to move between results. `Enter`
opens the selected item; `Alt-Enter` opens it in the background while keeping
the picker open. `Ctrl-v` opens a vertical split and `Ctrl-s` a horizontal one.
`Ctrl-t` toggles the preview. `Esc` closes the picker.

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

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
| `Space g` | Changed Git files |
| `Space '` | Reopen the last picker |

## Open a result

Use `Ctrl-n` and `Ctrl-p` or the arrow keys to move between results. `Enter`
opens the selected item; `Alt-Enter` opens it in the background while keeping
the picker open. `Ctrl-v` opens a vertical split and `Ctrl-s` a horizontal one.
`Ctrl-t` toggles the preview. `Esc` closes the picker.

Press `Ctrl-q` to copy matched file locations into the [quicklist](./quicklist.md)
for navigation after closing the picker. To customize keys, see
[Component keys](./remapping.md#component-keys).

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
keep navigating while updates load. Previews show current content, preferring
open buffers to files on disk. Global search also follows unsaved buffer edits.

Buffer and diagnostic pickers follow editor updates. A resolved diagnostic stays
visible while selected, marked as resolved, until you move away. Symbol pickers
refresh when their source documents or providers change. Reopening a picker
checks its current source again.

Quicklist and jump-history entries remain stored collections; filesystem changes
do not regenerate their contents.

If results do not update, check [file watching](./configuration.md#editorfileswatch)
and [periodic checks](./configuration.md#editorfilesreload).

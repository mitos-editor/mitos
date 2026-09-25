# Pickers

## Using pickers

Mitos has a variety of pickers, which are interactive windows used to select various kinds of items. These include a file picker, global search picker, and more. Most pickers are accessed via keybindings in [space mode](./keymap.md#space-mode). Pickers have their own [keymap](./keymap.md#picker) for navigation.

### Filtering Picker Results

Most pickers perform fuzzy matching using [fzf syntax](https://github.com/junegunn/fzf?tab=readme-ov-file#search-syntax). Two exceptions are the global search picker, which uses regex, and the workspace symbol picker, which passes search terms to the language server. Note that OR operations (`|`) are not currently supported.

If a picker shows multiple columns, you may apply the filter to a specific column by prefixing the column name with `%`. Column names can be shortened to any prefix, so `%p`, `%pa` or `%pat` all mean the same as `%path`. For example, a query of `mitos %p .toml !lang` in the global search picker searches for the term "mitos" within files with paths ending in ".toml" but not including "lang".

You can insert the contents of a [register](./registers.md) using `Ctrl-r` followed by a register name. For example, one could insert the currently selected text using `Ctrl-r`-`.`, or the directory of the current file using `Ctrl-r`-`%` followed by `Ctrl-w` to remove the last path section. The global search picker will use the contents of the [search register](./registers.md#default-registers) if you press `Enter` without typing a filter. For example, pressing `*`-`Space-/`-`Enter` will start a global search for the currently selected text.

Global search uses smart case by default: a lowercase pattern is case-insensitive, while an uppercase character makes the pattern case-sensitive. Press `Alt-c` to explicitly toggle between smart case and case-sensitive matching. You can also put `(?-i)` in a regex to force case-sensitive matching for that pattern, or set `editor.search.smart-case = false` to make all searches case-sensitive.

### Replacing global search results

Press `Alt-r` in global search to reveal and focus a replacement input. Once it is visible, `Alt-r` switches focus between the search and replacement inputs.

- `Enter` replaces the selected match.
- `Ctrl-Enter`, `Cmd-Enter`, or `Win-Enter` replaces all current matches after the search has finished.

Replacement text supports regex capture expansion: `$1` and `${name}` insert numbered and named captures, and `$$` inserts a literal dollar sign. Replacements update editor buffers and participate in normal undo history; they are not automatically saved.

Replaced matches are removed from the current result list without rerunning the search. When replacing one match, focus stays at the same position and moves naturally to the next result without changing the existing order.

To keep navigating a picker's current matched locations after closing it, press `Ctrl-q` to populate the [quicklist](./quicklist.md).

### File explorer

`Space-e` opens an interactive file explorer for browsing and opening files, rooted at the workspace; `Space-.` opens one rooted at the current buffer's directory. Unlike the file picker, the explorer does not ignore most files by default; its ignore behaviour is configured separately in the [`[editor.file-explorer]`](./editor.md#editorfile-explorer-section) section.

Supported images can be displayed in the preview pane. See
[Image previews](./image-previews.md) for formats and rendering details.

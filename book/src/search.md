# Search and replacement

## Find text in a file

Press `/`, type a regular expression, and press `Enter`. Use `n` and `N` to
select the next and previous matches. `?` starts a backward search.

To search for selected text, press `*`, then `n`. Mitos treats the text
literally and adds word boundaries where appropriate. Use `Alt-*` to search
without adding word boundaries.

Search ignores case unless the pattern contains an uppercase letter. It wraps
at the end of a file by default. Change either behavior in
[`editor.search`](./configuration.md#editorsearch).

## Replace several matches

1. Press `%` to select the whole file, or select the region you want to edit.
2. Press `s`, type the regular expression, and press `Enter`.
3. Press `Alt-r`, type the replacement, and press `Enter`.

Each match becomes a separate selection. The replacement applies to all of
them in one undo step. `Esc` cancels the replacement; an empty replacement
deletes the selected text. Replacement text is literal: `$1` does not expand a
regex capture. The new text stays selected. Press `,` to keep one selection,
or `.` to repeat the replacement on other selections.

Use `c` instead of `Alt-r` to edit the matches interactively in insert mode.
The `r` command replaces every selected character with one character:
selecting `hello` and pressing `rx` gives `xxxxx`; replacing it with `Alt-r`
and `x` gives `x`.

For an edit across several lines, `Alt-s` first creates one selection per
line. `K` keeps selections matching a regex; `Alt-K` removes matching
selections. See [Selections](./usage.md) for more examples.

## Search a project

Press `Space /` to open global search and enter a regular expression. Press
`Enter` to open a result. Use `Space '` to return to the results.

Filter paths by adding `%path` (or `%p`) to the query. For example,
`TODO %p .rs` searches for `TODO` in matching Rust file paths. Path filters use
fuzzy matching; see [picker filters](./pickers.md#filtering-results).

To search for the current selection, press `*`, then `Space /`, then `Enter`
without entering another query. Global search follows the
[file-picker ignore settings](./pickers.md#hidden-and-ignored-files).

Press `Ctrl-q` in the results to collect their locations in the
[quicklist](./quicklist.md). After closing the picker, use `]q` and `[q` to
visit each result and edit it. The selection replacement workflow above
operates within the current buffer.

To replace across files directly, press `Alt-r` in global search and enter a
replacement. `Enter` replaces one match; `Ctrl-Enter` replaces all occurrences
in the files matching the current path filter. Global replacement expands regex
captures and leaves changes unsaved. See [Replacing global search results](./pickers.md#replacing-global-search-results).

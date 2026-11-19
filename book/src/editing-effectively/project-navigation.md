# Navigate a project

Once you can [edit a region and repeat changes](./repeated-edits.md), the next
step is finding the right region in a larger project. Use filenames to find
files, text search to find occurrences, and symbols to find code definitions.

Open a project from its root directory:

```sh
ms .
```

## Find a file or a piece of code

Choose the picker based on what you know:

| What you know | Where to start |
| --- | --- |
| Part of a filename or path | `Space f`, then type part of the path |
| A file is already open | `Space b` to choose its buffer |
| Some text, but not its file | `Space /`, then type a regular expression |
| A function or type in the current file | `Space s` for file symbols |
| A symbol elsewhere in the project | `Space S` for workspace symbols |

For example, press `Space /`, type `TODO`, and inspect the preview before
pressing `Enter` on a result. To limit the results to Rust paths, use
`TODO %p .rs`. Path filters use fuzzy matching; `.rs$` requires that suffix.
See [Pickers](../pickers.md#filtering-results) for more filters.

File and workspace symbols use a language server when available, with syntax
queries as a fallback. Check [Language servers](../lsp.md#symbols-without-a-server)
for the setup and available language support.

## Follow a call and come back

While reading a function, you may want to inspect a call without losing your
place:

1. Press `Ctrl-s` in normal mode to save your location in the jumplist.
   This saves a location, not the file; use `:write` to save text.
2. Put the cursor on the called function and press `gd` to go to its definition
   when a language server provides it. Otherwise, use `Space /` to search for
   its name and press `Enter` on a result.
3. At the definition, use `Space k` for language-server documentation, or
   [textobjects](./textobjects.md) to select and read the function.
4. Press `Ctrl-o` to step back through jumps until you return to the caller.
   `Ctrl-i` steps forward again.

Use `ga` to switch between the two most recently accessed buffers. Use the
[jumplist](../jumplist.md) when you need to return to a position, including a
position within the same file.

## Keep two places visible

To keep the caller visible while reading another file:

1. Open a file, symbol, or search picker and choose the result.
2. Press `Ctrl-v` to open it in a vertical split.
3. Use `Ctrl-w h` and `Ctrl-w l` to move between the left and right views.
4. Press `Ctrl-w q` to close the focused view when you are finished.

Closing a view can leave its buffer open. `Space b` finds it again. Both views
show the same edits if they display the same buffer; they are not independent
copies. See [Files, buffers, and splits](../files.md).

## Keep a list of places to visit

For a task such as reviewing TODOs, press `Ctrl-q` in the search picker to
collect the matching locations in the quicklist. Close the picker with `Esc`,
then use `]q` and `[q` to visit the next and previous locations. `Space q` opens
the list.

Use a quicklist for a set of results you intend to work through. Use `Space '`
to reopen the last picker when you want to refine its query or browse again.
See [Quicklist](../quicklist.md) for more ways to collect locations.

**Previous:** [Repeat and combine edits](./repeated-edits.md) ·
**Next:** [Review and stage changes](./version-control.md)

# Files, buffers, and splits

A buffer holds an open document and its unsaved edits. A view displays a buffer.
You can open several buffers in one view, or show the same buffer in several
split views. Edits appear in every view displaying that buffer.

## Open and switch files

Start with one or more files from your shell:

```sh
ms README.md Cargo.toml
```

Inside Mitos, use `:open <path>`, the file picker (`Space f`), or the file
explorer (`Space e`). A path that does not exist opens a new buffer; `:write`
creates the file. Use `:write!` if its parent directories also need creating.

Press `Space b` to choose an open buffer. `gn` and `gp` switch to the next and
previous buffers; `ga` switches to the last accessed buffer.

Use `:new` to create an unnamed scratch buffer. Save it with
`:write notes.txt` when you want to keep it.

## Work in splits

| Keys | Action |
| --- | --- |
| `Ctrl-w v` | Split vertically, showing the current buffer |
| `Ctrl-w s` | Split horizontally, showing the current buffer |
| `Ctrl-w h` / `j` / `k` / `l` | Focus the split to the left / below / above / right |
| `Ctrl-w w` | Focus the next split |
| `Ctrl-w H` / `J` / `K` / `L` | Swap the current view with an adjacent view |
| `Ctrl-w q` | Close the current view |
| `Ctrl-w o` | Keep only the current view |

Press the keys in sequence, for example `Ctrl-w`, then `v`. `Space w` opens
the same window menu. To show a different file after splitting, use `Space b`
or `:open`. In a picker, `Ctrl-v` opens the selected file vertically and
`Ctrl-s` opens it horizontally.

You can also use `:vsplit <path>` or `:hsplit <path>`. To start with files in
separate views, see [Starting Mitos](./invocation.md).

Set [`editor.splits.strategy = "balanced"`](./configuration.md#editorsplits) to
keep pane widths or heights balanced when closing or rearranging splits.
Use `:set splits.strategy balanced` to rebalance existing panes immediately.

## Save and close

| Command | Action |
| --- | --- |
| `:write` / `:w` | Save the current buffer |
| `:write-all` / `:wa` | Save all modified buffers |
| `:buffer-close` / `:bc` | Close the current buffer |
| `:write-buffer-close` / `:wbc` | Save and close the current buffer |
| `:quit` / `:q` | Close the current view; exit when it is the last view |
| `:write-quit` / `:wq` | Save the current buffer and close the current view |
| `:write-quit-all` / `:wqa` | Save all modified buffers and exit |

Closing a view can leave its buffer available in the buffer picker. Closing a
buffer removes the document from the editor. Mitos refuses to discard unsaved
edits unless you use a force command such as `:buffer-close!` or `:quit-all!`.

## Changes made outside Mitos

Mitos reloads files with no unsaved edits when they change on disk. If you have
unsaved edits, it prompts before reloading. Use `:reload` to discard your edits
and read the file from disk yourself.

Configure [automatic reloads](./configuration.md#editorfilesreload),
[autosave](./configuration.md#editorfilesauto-save), or
[formatting on save](./languages.md#configuring-the-formatter-command) to suit
your workflow.

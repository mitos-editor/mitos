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

## Follow links

Press `gf` to follow a link or open a path in your selection.

Web links detected by your language server can also open in your browser with
your terminal's link-opening gesture, usually Ctrl-click or Cmd-click. This
requires a terminal that supports clickable links.

## Work in splits

| Keys | Action |
| --- | --- |
| `Ctrl-w v` | Split vertically, showing the current buffer |
| `Ctrl-w s` | Split horizontally, showing the current buffer |
| `Ctrl-w h` / `j` / `k` / `l` | Focus the split to the left / below / above / right |
| `Ctrl-w w` / `Ctrl-w W` | Focus the next / previous pane, including the visible sidebar |
| `Ctrl-w H` / `J` / `K` / `L` | Swap the current view with an adjacent view |
| `Ctrl-w q` | Close the current view |
| `Ctrl-w o` | Keep only the current view |

Press the keys in sequence, for example `Ctrl-w`, then `v`. `Space w` opens
the same window menu. To show a different file after splitting, use `Space b`
or `:open`. In a picker, `Ctrl-v` opens the selected file vertically and
`Ctrl-s` opens it horizontally.

You can also use `:split-vertical <path>` or `:split-horizontal <path>`. To start with files in
separate views, see [Starting Mitos](./invocation.md).

Press `Ctrl-e` to switch between the editor and the
[file explorer sidebar](./pickers.md#sidebar). Jumping left from a split at the
left edge also focuses the visible sidebar. In sidebar navigation, `Ctrl-w l`
returns to the last focused editor split; `Ctrl-w q` hides the sidebar.

### Resize windows

Press `Ctrl-w r` to resize a split or the file explorer sidebar. In the editor,
you can also use `Space w r`.

Use `h` / `j` / `k` / `l` or the arrow keys to move a border left / down / up /
right. Each press moves it 5 columns or 2 rows. Type a number before a direction
to choose the distance: `Ctrl-w r 10 h` moves a border 10 columns left.

Press `Enter` to keep the new sizes or `Esc` to cancel. Press `=` in resize mode
to make the splits equally sized and reset the sidebar width. `Ctrl-w =` makes
the splits equally sized without changing the sidebar.

The sidebar only changes width. You can resize it while it is focused or from
the buffer beside it.

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

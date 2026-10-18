# Multi-buffers

A multi-buffer combines editable excerpts from one or more source documents. Use
it to work on search results, diagnostics, symbols, or selected ranges together.

In a picker with file locations, press `Ctrl-o` to open the current matched
results as a multi-buffer. Each location includes three lines of surrounding
context. Overlapping or touching excerpts from the same source are merged.
To create one from the current selections, open the command picker (`Space-?`)
and choose `open_selections_in_multibuffer`.

Each excerpt displays its source filename and line numbers. Normal movement,
editing, and undo commands work in the combined buffer. Excerpts remain separate
sources even when you delete the text between their headers.

Save with `:w` to apply changed excerpts to their source documents and write those
files. Source edits outside the excerpts are preserved. If an excerpt has changed
in its source document, the write fails before applying any source edits. `:w!`
allows overwriting changed excerpts. Normal on-disk conflict and write-permission
checks still apply; `:w!` also overrides on-disk conflicts.

Multi-buffers are saved explicitly with `:w` or `:wa`; automatic saves skip them.
The multi-buffer remains modified until its source writes finish successfully.
If a write fails, correct the problem and retry. Writes across several files can
succeed or fail independently. Source documents must remain open and retain their
filenames. Sources without filenames need to be saved before their excerpts can
be written.

`Shift-Enter` adds three lines of context above and below the current excerpt.
Expansion preserves edits and merges touching excerpts from the same source. It
can be undone and does not itself mark a clean multi-buffer as modified.

| Command | Action |
| --- | --- |
| `extend_multibuffer_excerpt` | Expand above and below the cursor's excerpt |
| `expand_multibuffer_excerpt_up` | Expand upward |
| `expand_multibuffer_excerpt_down` | Expand downward |
| `goto_next_multibuffer_excerpt` | Move to the next excerpt's start |
| `goto_previous_multibuffer_excerpt_end` | Move to the previous excerpt's end |
| `open_multibuffer_excerpt_vsplit` | Open the source excerpt in a vertical split |

The optional theme scope `ui.multibuffer.header` styles source headers. Picker
bindings can be customized with the `open_multibuffer` action in `[keys.picker]`.

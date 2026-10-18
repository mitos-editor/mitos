# Basics

Mitos is a modal editor: the same keys navigate, select, and edit depending on
the current mode. This short walkthrough covers enough to edit a file without
trying to memorize the entire [keymap](./keymap.md).

For a guided lesson inside the editor, run `ms --tutor` or enter `:tutor` from
Mitos.

## Open a file

After [installing Mitos](./install.md), open a file or directory from your shell:

```sh
ms README.md
ms .
```

Mitos starts in **normal mode**. Press `Esc` whenever you want to return to it.

Key combinations such as `Ctrl-x` are pressed together. Sequences such as
`Space f` are pressed one key after another. Uppercase letters require Shift.

### Image previews

Open an image file the same way as a text file, for example `ms screenshot.png`.
See [Image previews](./image-previews.md) for supported formats and terminal rendering.

## Move around

Use the arrow keys or the home-row movement keys:

| Key | Movement |
| --- | --- |
| `h` / `j` / `k` / `l` | Left / down / up / right |
| `w` / `b` | Next / previous word start |
| `e` | Next word end |
| `Ctrl-u` / `Ctrl-d` | Half a page up / down |
| `gg` / `ge` | Start / end of the file |

You can prefix many movements with a count. For example, `5j` moves down five
visual lines.

## Insert text

From normal mode, press `i` to insert before the selection or `a` to append
after it. Type normally, then press `Esc` to finish the insertion.

Other useful entry points are `I` and `A` for the start and end of the line,
and `o` / `O` for a new line below or above.

## Select, then act

Mitos uses a **selection → action** editing model. A cursor is a one-character
selection; movement changes that selection. Press `v` to enter select mode,
where movement extends it, then apply an action:

| Key | Action |
| --- | --- |
| `d` | Delete the selection |
| `c` | Change the selection and enter insert mode |
| `y` | Yank (copy) the selection |
| `p` / `P` | Paste after / before the selection |
| `u` / `U` | Undo / redo |

Press `x` to select the current line. Textobjects, such as `miw`
for the word around the cursor, are described in [Textobjects](./textobjects.md).

## Edit several matches at once

Multiple selections are a normal part of editing, not a separate mode. One
simple workflow is:

1. Press `x` to select a line.
2. Press `s`, enter a regular expression, and press `Enter`.
3. Mitos creates one selection for every match in the line.
4. Press `Alt-r`, type the replacement, and press `Enter` to replace every selection.

Use `c` instead to change the matches interactively in insert mode. Press `,`
to keep only the primary selection. See [Search and replacement](./search.md)
for more examples and [Selections](./usage.md) for other ways to build selections.

## Search, save, and quit

Press `/` to search forward, then use `n` and `N` to move between matches.
Press `:` to enter command mode:

| Command | Result |
| --- | --- |
| `:write` or `:w` | Save the current file |
| `:quit` or `:q` | Close the current view |
| `:write-quit` or `:wq` | Save and close |
| `:quit-all!` or `:qa!` | Exit without saving unsaved changes |

## Where to go next

- Read [Files, buffers, and splits](./files.md) to work with several documents.
- Keep the [Keymap](./keymap.md) open as a reference while learning.
- Set up [language servers](./lsp.md) for completion, diagnostics, and code actions.
- Adjust editor behavior in [Configure Mitos](./editor.md).

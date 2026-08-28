---
title: Basics
description: Learn the editing model and make your first changes in Mitos.
---

Mitos is a modal editor: the same keys navigate, select, and edit depending on
the current mode. This short walkthrough covers enough to edit a file without
trying to memorize the entire [keymap](../keymap/).

For a guided lesson inside the editor, run `ms --tutor` (or enter `:tutor` from
Mitos).

## Open a file

After [installing Mitos](../install/), open a file or directory from your shell:

```sh
ms README.md
ms .
```

Mitos starts in **normal mode**. Press `Esc` whenever you want to return to it.

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

Press `x` to select the current line. Syntax-aware text objects, such as `miw`
for the word around the cursor, are described in [Textobjects](../textobjects/).

## Edit several matches at once

Multiple selections are a normal part of editing, not a separate mode. One
simple workflow is:

1. Press `x` to select a line.
2. Press `s`, enter a regular expression, and press `Enter`.
3. Mitos creates one selection for every match in the line.
4. Press `c` and type once to replace every selection.

Use `,` to keep only the primary selection. The [Usage](../usage/) and
[Registers](../registers/) pages cover more ways to compose edits.

## Search, save, and quit

Press `/` to search forward, then use `n` and `N` to move between matches.
Press `:` to enter command mode:

| Command | Result |
| --- | --- |
| `:write` or `:w` | Save the current file |
| `:quit` or `:q` | Close the current view |
| `:write-quit` or `:wq` | Save and close |
| `:quit!` or `:q!` | Close and discard unsaved changes |

## Where to go next

- Read [Usage](../usage/) for modes, buffers, selections, and motions.
- Keep the [Keymap](../keymap/) open as a reference while learning.
- Set up [language servers](../lsp/) for completion, diagnostics, and code actions.
- Adjust editor behavior in [Configuration](../configuration/).

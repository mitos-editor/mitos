# Selections

A cursor is a one-character selection. In normal mode, motions such as `w`
change the selection; in select mode (`v`), they extend it. Press `Esc` to
return to normal mode. See [Basics](./basics.md) for your first edits.

## Build multiple selections

The same editing commands apply to one selection or many:

| Key | Action |
| --- | --- |
| `s` | Select regex matches within the current selections |
| `S` | Split selections at regex matches |
| `Alt-s` | Split selections at newlines |
| `C` / `Alt-C` | Add a selection on the line below / above |
| `K` / `Alt-K` | Keep / remove selections matching a regex |
| `,` | Keep only the primary selection |
| `Alt-,` | Remove the primary selection |

For example, select several lines, press `Alt-s`, then `I` to insert at the
start of each selected line. Type a prefix and press `Esc` to finish.
For matching text, see [Search and replacement](./search.md).

## Adjust selections

The primary selection is the one used by commands that need a single position,
such as showing documentation. Use `(` and `)` to change which selection is
primary.

Each selection has an anchor and a cursor. Motions in select mode move the
cursor while leaving the anchor in place. `Alt-;` swaps the two ends, `;`
collapses each selection to its cursor, and `Alt-:` makes selections forward.

Use `x` to select a line, `%` to select the whole buffer, and `miw` to select
inside a word. [Textobjects](./textobjects.md) and
[syntax-aware motions](./syntax-aware-motions.md) select larger structures.

## Transform selected text

`d`, `c`, `y`, and `Alt-r` operate on every selection. `J` joins selected
lines, `_` trims whitespace from selections, and `&` aligns selections in
columns. `Alt-(` and `Alt-)` rotate the selected contents between locations.

`Ctrl-j` and `Ctrl-k` move selected lines down and up. Adjacent selected lines
move as one block; a count such as `3 Ctrl-j` moves it three lines in one undo
step. Blocks stop at document edges. A multiline block ending in a blank line also stops
before a final line without a newline, keeping that blank line in the selection.

See the [selection keymap](./keymap.md#selection-manipulation) for the full list,
[Registers](./registers.md) for copying text, and
[Shell commands](./shell.md) for filtering selections through other tools.

# Migrating from Vim

Mitos uses selection-first editing: select the text, then apply an action.
For example, Vim's `dw` becomes `wd`. A cursor is a one-character selection.
See [Basics](./basics.md) for a walkthrough or run `ms --tutor`.

## Delete/Change Commands

| Task | Vim | Mitos |
| --- | --- | --- |
| Delete a word | `dw` | `wd` |
| Change a word | `cw` | `ec`, or `wc` including following whitespace |
| Delete a character | `x` | `d`; use `;d` to collapse an existing selection first |
| Copy a line | `yy` | `xy` |
| Replace throughout a file | `:%s/word/replacement/g` | `%`, `s`, type `word`, `Enter`, `Alt-r`, type the replacement, `Enter` |

Replacement text is literal; `$1` does not expand a regex capture. See
[Search and replacement](./search.md) for selection and project searches.

## Navigation

| Task | Vim | Mitos |
| --- | --- | --- |
| First / last line | `gg` / `G` | `gg` / `ge` |
| Line start / end | `0` / `$` | `gh` / `gl` |
| First nonblank character | `^` | `gs` |
| Matching bracket | `%` | `mm` |
| Alternate file | `Ctrl-^` | `ga` |

Unlike Vim, `f`, `F`, `t`, and `T` can search across line boundaries.
Mitos's `gw` displays labels at words: type a label to jump to that word.

## Line Deletes

| Task | Vim | Mitos |
| --- | --- | --- |
| Delete to line end | `D` | `vgld` |
| Delete a whole line | `dd` | `xd` |

`v` extends the selection as you move; `x` selects the current line.

## Comment lines, Completion, Search

| Task | Mitos |
| --- | --- |
| Completion while inserting | `Ctrl-x` |
| Toggle comments | `Space c` |
| Search for a word | `miw`, `*`, then `n` |
| Add a cursor below | `C` |
| Keep only one selection | `,` |

For a rectangular edit, add cursors with `C` and extend the selections using
normal motions. See [Selections](./usage.md) and
[Completion and snippets](./completion.md).

## File actions

Use `Space f` for files and `Space b` for open buffers. `Ctrl-w` opens the
window menu; see [Files, buffers, and splits](./files.md).

`:reload` reads the current file from disk and discards unsaved edits.
`:sh <command>` runs a shell command; `!` inserts command output. See
[Shell commands](./shell.md).

Mitos uses the [jumplist](./jumplist.md) for saved locations: `Ctrl-s` saves
one, `Ctrl-o` goes back, `Ctrl-i` goes forward, and `Space j` opens the list.

See [Textobjects](./textobjects.md), [Surround](./surround.md), and
[Registers](./registers.md) for other familiar operations with different keys.

The cursor is a block in every mode by default. To use a bar while inserting:

```toml
[editor.cursor.shape]
insert = "bar"
```

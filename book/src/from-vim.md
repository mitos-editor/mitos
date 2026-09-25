# Migrating from Vim

- [Delete/Change Commands](#deletechange-commands)
- [Navigation](#navigation)
- [Line Deletes](#line-deletes)
- [Comment lines, Completion, Search](#comment-lines-completion-search)
- [File actions](#file-actions)

Mitos's editing model is strongly inspired from Vim and Kakoune, and a notable
difference from Vim (and the most striking similarity to Kakoune) is that Mitos
follows the `selection → action` model. This means that whatever you are
going to act on (a word, a paragraph, a line, etc.) is selected first and the
action itself (delete, change, yank, etc.) comes second. A cursor is simply a
single width selection.

*Note:* As Mitos is inspired by Vim and [Kakoune](https://github.com/mawww/kakoune), the keybindings are similar but also have some differences. The content of this page is inspired by [Kakoune Wiki](https://github.com/mawww/kakoune/wiki/Migrating-from-Vim).

NOTE: Unlike vim, `f`, `F`, `t` and `T` are not confined to the current line.

## Delete/Change Commands

delete a word:
* vim: `dw`
* mitos: `wd`

change a word:
* vim: `cw`
* mitos: `ec` or `wc` (includes the whitespace after the word)

delete a character:
* vim: `x`
* mitos: `d` or `;d` (`;` reduces the selection to a single char)

copy a line:
* vim: `yy`
* mitos: `Xy` (`X` extends all selections to whole lines)

global replace:
* vim: `:%s/word/replacement/g<ret>`
* mitos: `%sword<ret><A-r>replacement<ret>`

Explanation: `%` selects the entire buffer, `s` opens a regex prompt, and `<ret>` selects each match. `Alt-r` opens the replacement prompt. Type the replacement and press `<ret>` to apply it to all selections in one undo step. Replacement text is literal; `$1` does not expand a regex capture.

## Navigation

go to first line:
* vim: `gg`
* mitos: `gg`

go to last line:
* vim: `G`
* mitos: `ge`

go to line start:
* vim: `0`
* mitos: `gh`

go to line first non-blank character:
* vim: `^`
* mitos: `gs`

go to line end:
* vim: `$`
* mitos: `gl`

jump to matching bracket:
* vim: `%`
* mitos: `mm`

## Line Deletes

delete to line end:
* vim: `D`
* mitos: `vgld` or `t<ret>d`

Note: `v` is used along with `gl` (go to line end), because [`gl` does not select text](https://github.com/helix-editor/helix/issues/1630).
`t<ret>` selects "'til" the newline represented by `<ret>`.

delete entire line:
* vim: `dd`
* mitos: `xd`

Note: `x` selects the entire line under the cursor

## Comment lines, Completion, Search

auto complete:
* vim: `C-p`
* mitos: `C-x`

comment lines:
* vim: `gc`
* mitos: `Space-c`

search for the word under the cursor:
* vim: `*`
* mitos: `A-o*n` (if there's a tree-sitter grammar or LSP) or `be*n`

Explanation: if there's a grammar or LSP, `A-o` expands selection to the parent syntax node (which would be the word in our case). Then `*` uses the current selection as the search pattern, and `n` goes to the next occurrence. `b` selects to the beginning of the word, and `e` selects to the end of the word, effectively selecting the whole word.

block selection:
* vim: `C-v`, then expand your selection vertically and horizontally
* mitos: There's no "block selection" mode, so instead you'd use multiple cursors. Expand your block selection vertically by adding new cursors on the line below with `C`, and horizontally using standard movements

search "foo" and replace with "bar" in the current selection:
* vim: `:s/foo/bar/g<ret>`
* mitos: `sfoo<ret><A-r>bar<ret>,`

Explanation: `s` opens a regex prompt and selects all matches inside the selection. `Alt-r` opens the replacement prompt; enter `bar` and press `<ret>` to apply it once to each selection. Keep only the main selection with `,`.

## File actions

select the whole file:
* vim: `ggVG`
* mitos: `%`

reload a file from disk:
* vim: `:e<ret>`
* mitos: `:reload<ret>` (or `:reload-all<ret>` to reload all the buffers)

run shell command:
* vim: `:!command`
* mitos: `:sh command` (or `!command` to insert its output into the buffer)

setting a bookmark (bookmarking a location):
* vim: `ma` to set bookmark with name a. Use `` `a `` to go back to this bookmarked location.
* mitos: there are no named bookmarks, but you can save a location in the jumplist with `C-s`, then jump back to that location by opening the jumplist picker with `<space>-j`, or back in the jumplist with `C-o` and forward with `C-i`

Mitos allows [some limited movement in `insert` mode](https://docs.helix-editor.com/keymap.html#insert-mode) without switching to `normal` mode.

Unlike Vim, under Mitos, the cursor shape is the same (block) in insert mode and normal mode by default.
This can be adjusted in configuration:

```toml
[editor.cursor-shape]
insert = "bar"
```

> TODO: Mention textobjects, surround, registers


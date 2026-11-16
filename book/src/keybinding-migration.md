# Keybinding migration

These defaults contain breaking changes. Update custom bindings and recorded
macros using the tables below. Configuration loading does not rewrite them.
The selection-first editing model stays the same.

## Editor changes

| Previous key or behavior | New default |
| --- | --- |
| `Ctrl-s` captured a jump in normal/select mode | Save the current buffer in normal, select, and insert mode |
| `Ctrl-s` created an undo checkpoint in insert mode | `Ctrl-g u` |
| `Space j` opened the jumplist | `Space j j`; `Space j s` captures the current location |
| `Space w` opened window commands | Save; `Space W` saves all modified buffers. Window commands use `Ctrl-w` |
| Bare `G` did nothing | Go to the last line; a count selects that line, as in `42G` |
| `G`, `gg`, `zn`, and `Zn` could lose select-mode behavior | Line jumps extend the selection; view-mode search aliases add matches |
| `Space /` searched the working directory | Search the resolved workspace root, matching `Space f`; `search_picker_in_current_working_directory` still searches the working directory |
| `Space i` / `Space B` were Git aliases | Use `Space g i` / `Space g t` |

Saving and other non-text commands reject completion previews before running.

Function keys provide additional entry points: `F2` renames, `F12` goes to a
definition, `F5` continues debugging, `F9` toggles a breakpoint, and `F10` /
`F11` / `Shift-F11` step over / into / out. The modal bindings remain available
when a terminal or operating system reserves these keys.

# Keybinding migration

These defaults contain breaking changes. Update custom bindings and recorded
macros using the tables below. Configuration loading does not rewrite them.
The selection-first editing model stays the same.

## Find and inspect bindings

In the editor, `Ctrl-g k` opens a searchable list of active bindings and
`Ctrl-g i` inspects a key sequence without executing it. Both commands are also
available as `keybindings` and `inspect_key` in the command palette. In a picker
or prompt, `F1` opens the list and `Ctrl-x i` opens the inspector. `F1` in the
editor opens the command palette.

The list includes the focused component and the editor mode beneath it. Each row
shows its action, source, and whether another context shadows it. Select a row
to see the winning context, built-in/user/workspace definitions, and pending
prefix. The inspector captures the context that was active when it opened;
close and reopen it after changing focus or configuration.

Normal and select maps are initialized separately. A custom normal-mode remap
does not automatically change select mode. Remap both when appropriate.
Picker-specific maps inherit `[keys.picker]`; sidebar maps inherit
`[keys.file-explorer]`. A specific binding overrides an inherited binding.

## Editor changes

| Previous key or behavior | New default |
| --- | --- |
| `Ctrl-s` captured a jump in normal/select mode | Save the current buffer in normal, select, and insert mode |
| `Ctrl-s` created an undo checkpoint in insert mode | `Ctrl-g u` |
| `Space j` opened the jumplist | `Space j j`; `Space j s` captures the current location |
| `Space w` opened window commands | Save; `Space W` saves all modified buffers. Window commands use `Ctrl-w` |
| `Ctrl-c` toggled comments | Cancel the innermost input state; use `Space c` for comments |
| Bare `G` did nothing | Go to the last line; a count selects that line, as in `42G` |
| `G`, `gg`, `zn`, and `Zn` could lose select-mode behavior | Line jumps extend the selection; view-mode search aliases add matches |
| `Space /` searched the working directory | Search the resolved workspace root, matching `Space f`; `search_picker_in_current_working_directory` still searches the working directory |
| `Space G` opened sticky debug mode | `Space x` runs one debugger command; `Space X` stays in debug mode |
| Debug `Ctrl-c` / `l` configured a breakpoint | Debug `B c` / `B l`; `Ctrl-c` cancels |
| `Space i` / `Space B` were Git aliases | Use `Space g i` / `Space g t` |

`Esc` and `Ctrl-c` first cancel a pending prefix, literal input, completion menu,
popup, or resize operation. With a completion menu open they reject the preview
and stay in insert mode. Press either again to leave insert mode. Saving and
other non-text commands reject the completion preview before running.

Function keys provide additional entry points: `F2` renames, `F12` goes to a
definition, `F5` continues debugging, `F9` toggles a breakpoint, and `F10` /
`F11` / `Shift-F11` step over / into / out. The modal bindings remain available
when a terminal or operating system reserves these keys.

## Folds and views

| Previous | New |
| --- | --- |
| `zfc` / `zfo` / `zfa` | `zc` / `zo` / `za`: close / open / toggle a fold |
| `zfs` | `zf`: fold the selection |
| `zfm` / `zfr` | `zM` / `zR`: close / open all folds |
| `zc` centered the cursor | `zz` |

Sticky view mode uses the same suffixes: `Zc`, `Zo`, `Za`, `Zf`, `ZM`, `ZR`,
and `Zz`. `Esc` or `Ctrl-c` leaves sticky view or debug mode.

## Picker and prompt changes

Frequent actions keep direct shortcuts. Press `Ctrl-x` for the full action menu,
including equivalent routes for these actions. The footer prefers direct
shortcuts from your active bindings and reserves space for the menu hint.
Result navigation uses arrows, `Ctrl-n` / `Ctrl-p`, `Tab` / `Shift-Tab`, and
Page Up / Page Down.

| Action | Direct shortcut | Menu route |
| --- | --- | --- |
| Open / open while keeping picker | Enter / Alt-Enter | Enter / `Ctrl-x Enter` |
| Open horizontal / vertical split | `Ctrl-s` / `Ctrl-v` | `Ctrl-x s` / `Ctrl-x v` |
| Toggle preview | `Ctrl-t` | `Ctrl-x p` |
| Copy results to quicklist | `Ctrl-q` | `Ctrl-x q` |
| Focus replacement / toggle search case | `Alt-r` / `Alt-c` | `Ctrl-x r` / `Ctrl-x f` |
| Replace all, when replacement is visible | `Ctrl-Enter` | `Ctrl-x R` |
| Git stage / unstage | `Alt-s` / `Alt-u` | `Ctrl-x a s` / `Ctrl-x a u` |
| Create / move a file in the explorer | `Alt-n` / `Alt-m` | `Ctrl-x n` / `Ctrl-x r` |

`Ctrl-s` saves in the editor's normal, select, and insert modes. A focused picker
uses it to open the selected result in a horizontal split.

Other picker changes:

| Previous | New |
| --- | --- |
| `Ctrl-l`: pause / continue / retry loading | `Ctrl-x l` |
| Home / End: first / last result | `Ctrl-x g` / `Ctrl-x G` |
| Explorer `Alt-c` / `Alt-d` / `Alt-y`: copy / delete / yank | `Ctrl-x c` / `Ctrl-x d` / `Ctrl-x y` |
| `Ctrl-x`: remove a quicklist row | `Ctrl-x d`; the buffer picker also uses it to close a buffer |
| Git `Alt-z` / `Alt-l`: stash / browse stashes | `Ctrl-x a z` / `Ctrl-x a l` |
| Git or stash `Alt-a`: action chooser | `Ctrl-x a ?` |
| Stash `Alt-s` / `Alt-i`: apply / apply with index | `Ctrl-x a s` / `Ctrl-x a i` |
| Branch/worktree `Alt-n` / `Alt-d` | `Ctrl-x n` / `Ctrl-x d` |
| Worktree `Alt-l` / `Alt-u`: lock / unlock | `Ctrl-x a l` / `Ctrl-x a u` |
| History `Alt-a` / `Alt-h`: actions / back | `Ctrl-x a` / `Ctrl-x b` |

Home and End edit the query. `Ctrl-d` deletes forward, `Ctrl-u` deletes to the
start, and `Alt-d` deletes the next word in every picker query. `Alt-w` inserts
the word under the editor cursor. Use `Ctrl-x h` / `Ctrl-x H` for previous / next
query history.

While navigating the sidebar, `n`, `r`, `c`, `d`, and `y` create, move, copy,
delete, and yank the selected path. While filtering, those letters enter query
text. `Ctrl-x` actions work in both states; deletion still asks for confirmation.

Preview scrolling keeps `Alt-j` / `Alt-k` and adds `Ctrl-x j` / `Ctrl-x k`.
Review previews support `Ctrl-x [` / `Ctrl-x ]` for previous / next comparison
and `Ctrl-x o` to open it. Menu sequences also provide alternatives when a
terminal cannot distinguish modified Enter or transmit an Alt shortcut.

## Completion, snippets, and Tab

`Tab` and `Enter` accept the selected completion, or the first match if none is selected. Use `Ctrl-n` / `Ctrl-p` or arrows to
select an item; `Shift-Tab` also selects the previous item. An ordinary typed
character can accept a selected preview before inserting the character.
Commands such as save, movement, help, and cancellation do not accept it.

Inside an active snippet, `Tab` / `Shift-Tab` move between placeholders even on
whitespace-only lines. `Ctrl-g n` / `Ctrl-g p` cycle placeholder choices.
`Ctrl-g s` shows signature help. Without a completion or snippet, `Tab` indents
and `Shift-Tab` unindents. Syntax-aware smart Tab is opt-in:

```toml
[editor.indent.smart-tab]
enable = true
```

Inline completion remains opt-in and has no default acceptance key. See
[Completion and snippets](./completion.md) for explicit remapping examples.

## Terminal input and runtime states

Terminals may encode `Ctrl-i` as Tab, `Ctrl-m` as Enter, or `Ctrl-[` as Escape.
The inspector reports the event Mitos receives; it cannot distinguish physical
keys that the terminal encodes identically. Use the portable prefix sequences
above if modified keys do not reach the editor.

Some inputs depend on the current operation: digits build a count, `.` repeats
an edit, register/delimiter prompts take one literal character, and resize mode
accepts directions and distances. The binding inspector includes these active
input states. Resize ends with Enter and cancels with Escape or `Ctrl-c`.

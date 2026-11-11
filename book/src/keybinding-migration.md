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

Press `Ctrl-x` in a picker to see its available actions. Prefix menus and the
binding list omit actions the current picker cannot perform. Result navigation
uses arrows, `Ctrl-n` / `Ctrl-p`, `Tab` / `Shift-Tab`, and Page Up / Page Down.

| Previous | New |
| --- | --- |
| `Ctrl-s` / `Ctrl-v`: split below / right | `Ctrl-x s` / `Ctrl-x v` |
| `Alt-Enter`: open and keep picker | Also available as `Ctrl-x Enter` |
| `Ctrl-t`: toggle preview | `Ctrl-x p` |
| `Ctrl-l`: pause / continue / retry loading | `Ctrl-x l` |
| `Ctrl-q`: copy results to quicklist | `Ctrl-x q` |
| Home / End: first / last result | `Ctrl-x g` / `Ctrl-x G` |
| `Alt-r` / `Alt-c`: replacement input / case toggle | `Ctrl-x r` / `Ctrl-x f` in search pickers |
| `Ctrl-Enter`: replace all | Also available as `Ctrl-x R` when replacement is visible |
| `Alt-n` / `Alt-m` / `Alt-c` / `Alt-d` / `Alt-y`: file actions | `Ctrl-x n` / `r` / `c` / `d` / `y`: create / move / copy / delete / yank path |
| `Ctrl-x`: remove a quicklist row | `Ctrl-x d`; the buffer picker also uses it to close a buffer |
| Git `Alt-s` / `Alt-u` / `Alt-z` / `Alt-l` | `Ctrl-x a s` / `u` / `z` / `l`: stage / unstage / stash / browse stashes |
| Git or stash `Alt-a`: action chooser | `Ctrl-x a ?` |
| Stash `Alt-s` / `Alt-i`: apply / apply with index | `Ctrl-x a s` / `Ctrl-x a i` |
| Branch/worktree `Alt-n` / `Alt-d` | `Ctrl-x n` / `Ctrl-x d` |
| Worktree `Alt-l` / `Alt-u`: lock / unlock | `Ctrl-x a l` / `Ctrl-x a u` |
| History `Alt-a` / `Alt-h`: actions / back | `Ctrl-x a` / `Ctrl-x b` |

Home and End now edit the query. `Ctrl-d` deletes forward, `Ctrl-u` deletes to
the start, and `Alt-d` deletes the next word in every picker query. `Ctrl-s`
shows a reminder to return to the editor to save. `Alt-w` inserts the word under
the editor cursor. Use `Ctrl-x h` / `Ctrl-x H` for previous / next query history.

Preview scrolling keeps `Alt-j` / `Alt-k` and adds `Ctrl-x j` / `Ctrl-x k`.
Review previews support `Ctrl-x [` / `Ctrl-x ]` for previous / next comparison
and `Ctrl-x o` to open it. These sequences provide alternatives to terminal Alt
and modified Enter encodings.

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

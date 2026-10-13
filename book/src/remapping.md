# Key remapping

There are three kinds of commands that can be used in keymaps:

* Static commands: commands like `move_char_right` which are usually bound to
  keys and used for movement and editing. A list of static commands is
  available in the [Keymap](./keymap.md) documentation and in the source code
  in [`crates/term/src/commands.rs`](https://github.com/mitos-editor/mitos/blob/main/crates/term/src/commands.rs)
  at the invocation of `static_commands!` macro.
* Typable commands: commands that can be executed from command mode (`:`), for
  example `:write!`. See the [Commands](./commands.md) documentation for a
  list of available typeable commands or the `TypableCommandList` declaration in
  the source code at [`crates/term/src/commands/typed.rs`](https://github.com/mitos-editor/mitos/blob/main/crates/term/src/commands/typed.rs).
* Macros: sequences of keys that are executed in order. These keybindings
  start with `@` and then list any number of keys to be executed. For example
  `@miw` can be used to select the surrounding word. For now, macro keybindings
  are not allowed in keybinding sequences due to limitations in the way that
  command sequences are executed. Modifier keys (e.g. Alt+o) can be used
  like `"<A-o>"`, e.g. `"@miw<A-o>"`

To remap keys, create a `config.toml` file in your `mitos` configuration
directory (default `~/.config/mitos` on Linux systems) with a structure like
this:

> 💡 To set a modifier + key as a keymap, type `A-X = ...` or  `C-X = ...` for Alt + X or Ctrl + X. Combine with Shift using a dash, e.g. `C-S-esc`. 
> Within macros, wrap them in `<>`, e.g. `<A-X>` and `<C-X>` to distinguish from the `A` or `C` keys.

```toml
# At most one section each of 'keys.normal', 'keys.insert' and 'keys.select'
[keys.normal]
C-s = ":w" # Maps Ctrl-s to the typable command :w which is an alias for :write (save file)
C-o = ":open ~/.config/mitos/config.toml" # Maps Ctrl-o to opening of the mitos config file
a = "move_char_left" # Maps the 'a' key to the move_char_left command
w = "move_line_up" # Maps the 'w' key move_line_up
"C-S-esc" = "extend_line" # Maps Ctrl-Shift-Escape to extend_line
g = { a = "code_action" } # Maps `ga` to show possible code actions
"ret" = ["open_below", "normal_mode"] # Maps the enter key to open_below then re-enter normal mode
"A-x" = "@x<A-d>" # Maps Alt-x to a macro selecting the whole line and deleting it without yanking it

# Label a nested menu and its typable commands, macros, or command sequences
[keys.normal.space.f]
label = "File"
f = "file_picker"
s = { label = "Save", command = ":write" }
c = { label = "Edit Config", command = ":open ~/.config/mitos/config.toml" }
w = { label = "Delete word", command = "@wd" }
h = { label = "Split left", command = ["vsplit", "normal_mode", "swap_view_left"] }

[keys.insert]
"A-x" = "normal_mode"     # Maps Alt-X to enter normal mode
j = { k = "normal_mode" } # Maps `jk` to exit insert mode
```

`label` sets the menu title or the description shown for a command binding.
Menus can be nested and labeled at any depth. Labels also override the names of
built-in menus while keeping their other bindings. Trusted workspace labels
override global labels; omitting a label or setting it to an empty string keeps
the existing menu title or command description.

The `command` field accepts a command string or an array of commands. A binding
cannot contain both `command` and nested keybindings. Individual static commands
keep their built-in descriptions; sequences may contain static and typable
commands and have their own label. Macros cannot be used in command sequences.
These labels apply to editor mode maps (`normal`, `insert`, and `select`).

## Minor modes

Minor modes are accessed by pressing a key (usually from normal mode), giving access to dedicated bindings. Bindings
can be modified or added by nesting definitions.

```toml
[keys.insert.j]
k = "normal_mode" # Maps `jk` to exit insert mode

[keys.normal.g]
a = "code_action" # Maps `ga` to show possible code actions

# invert `j` and `k` in view mode
[keys.normal.z]
j = "scroll_up"
k = "scroll_down"

# create a new minor mode bound to `+`
[keys.normal."+"]
m = ":run-shell-command make"
c = ":run-shell-command cargo build"
t = ":run-shell-command cargo test"
```

## Component keys

Prompts, pickers, menus, and popups have their own action maps. Hover pages and
signature overloads can also be remapped. Each table accepts the actions listed
below; action names are local to that component.

```toml
[keys.picker]
C-j = "move_down"
C-k = "move_up"
C-space = "accept"
C-n = "unbound" # Release this key to the picker's prompt.

[keys.buffer-picker]
C-x = "close_buffer" # Close the selected buffer and keep the picker open.

[keys.prompt]
C-j = "accept"
C-x = { h = "move_start", l = "move_end" }
A-backspace = ["delete_word_backward", "delete_word_backward"]

[keys.menu]
C-j = "move_down"
C-k = "move_up"

[keys.popup]
C-j = "scroll_down"
C-k = "scroll_up"

[keys.signature-help]
A-j = "next"
A-k = "previous"

[keys.hover]
A-j = "next"
A-k = "previous"
```

Bindings merge over the existing defaults. Trusted workspace configuration
overrides global configuration, including individual keys in nested maps.
`:config-reload` applies changes to open components as well.

`no_op` consumes a key without performing an action. `unbound` removes its
component action and lets the usual fallback handle the key: printable text in
a prompt, the embedded prompt in a picker, or the next component for menus and
popups. Nested content has priority over its container: completion menus use
`keys.menu` before `keys.popup`. To release `C-d` from both layers to an editor
binding, set `C-d = "unbound"` in both tables.

Nested maps wait for another key. Escape or an invalid continuation cancels
the sequence and consumes that key. Prefix state belongs to each component
instance. Pasting into a prompt cancels its prefix and inserts the pasted text.
Action lists run in order and stop when an action closes the component, starts
register selection, or performs an item action. Editor commands and macros belong
to the editor mode maps and cannot be used as component actions.

Picker actions take priority over prompt actions. Prompt editing bindings apply
to a picker's query when the picker leaves that key unbound; use picker actions
to accept or cancel a picker. Smart Tab still supersedes default menu Tab
navigation when enabled, while explicit `accept` or `no_op` bindings take effect.
Insert-mode popups normally pass Escape through to the editor; Escape cancels
pending component sequences first, and an explicit popup `no_op` binding consumes
it. To remap Escape inside an insert-mode menu or page selector, also set
`esc = "unbound"` in `keys.popup` so the container passes it to its content.

| Table | Actions |
| --- | --- |
| `keys.prompt` | `accept`, `cancel`, `move_left`, `move_right`, `move_word_left`, `move_word_right`, `move_start`, `move_end`, `delete_backward`, `delete_forward`, `delete_word_backward`, `delete_word_forward`, `delete_to_start`, `delete_to_end`, `insert_word`, `insert_register`, `history_previous`, `history_next`, `complete_previous`, `complete_next`, `exit_selection` |
| `keys.picker` | `accept`, `accept_keep_open`, `accept_horizontal_split`, `accept_vertical_split`, `cancel`, `move_up`, `move_down`, `move_start`, `move_end`, `page_up`, `page_down`, `toggle_preview`, `populate_quicklist` |
| `keys.buffer-picker` | All `keys.picker` actions, plus `close_buffer` |
| `keys.menu` | `accept`, `cancel`, `move_up`, `move_down`, `page_up`, `page_down` |
| `keys.popup` | `cancel`, `scroll_up`, `scroll_down` |
| `keys.hover`, `keys.signature-help` | `previous`, `next` |

All component tables also accept `no_op` and `unbound`. Popup scrolling and menu
paging move by half a page. Picker paging moves by a full page.

`keys.buffer-picker` inherits `keys.picker` and accepts all shared picker actions
plus `close_buffer`. Buffer-specific bindings override shared bindings, including
individual keys in nested maps. A workspace override replaces a global binding
within the same table; buffer-specific bindings take priority over shared bindings
from either configuration file.

`close_buffer` operates on the selected row. It waits for pending saves, refuses
to discard unsaved changes, and reports errors while leaving the picker open.
On success, it refreshes the buffer list, keeps the query, and selects the nearest
remaining row. It also ends the current action sequence so subsequent actions
cannot use a selection that is still being refreshed. The action is available
only in `keys.buffer-picker`; it has no default binding.
The picker accepts another row once the refreshed results are ready; query editing
and cancellation remain available while the matcher runs.

## Special keys and modifiers

Ctrl, Shift and Alt modifiers are encoded respectively with the prefixes `C-`, `S-` and `A-`.

The [Super key](https://en.wikipedia.org/wiki/Super_key_(keyboard_button)) - the Windows/Linux
key or the Command key on Mac keyboards - is also supported when using a terminal emulator that
supports the [enhanced keyboard protocol](https://github.com/helix-editor/helix/wiki/Terminal-Support#enhanced-keyboard-protocol).
The super key is encoded with prefixes `Meta-`, `Cmd-` or `Win-`. These are all synonyms for the
super modifier - binding a key with a `Win-` modifier will mean it can be used with the
Windows/Linux key or the Command key.

```toml
[keys.normal]
C-s = ":write" # Ctrl and 's' to write
Cmd-s = ":write" # Cmd or Win or Meta and 's' to write
```

Special keys are encoded as follows:

| Key name         | Representation |
| ---              | ---            |
| Backspace        | `"backspace"`  |
| Space            | `"space"`      |
| Return/Enter     | `"ret"`        |
| Left             | `"left"`       |
| Right            | `"right"`      |
| Up               | `"up"`         |
| Down             | `"down"`       |
| Home             | `"home"`       |
| End              | `"end"`        |
| Page Up          | `"pageup"`     |
| Page Down        | `"pagedown"`   |
| Tab              | `"tab"`        |
| Delete           | `"del"`        |
| Insert           | `"ins"`        |
| Null             | `"null"`       |
| Escape           | `"esc"`        |
| Less Than (<)    | `"lt"`         |
| Greater Than (>) | `"gt"`         |

Keys can be disabled by binding them to the `no_op` command.

All other keys such as `?`, `!`, `-` etc. can be used literally:

```toml
[keys.normal]
"?" = ":write"
"!" = ":write"
"-" = ":write"
```

Note: `-` can't be used when combined with a modifier, for example `Alt` + `-` should be written as `A-minus`. `A--` is not accepted.

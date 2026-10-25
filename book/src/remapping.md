# Key remapping

Keybindings can run:

- **Static commands**, such as `move_char_right`, for movement and editing.
- **Typable commands**, such as `:write`, which also work from command mode.
- **Macros**, such as `@miw`, which replay key sequences.

See [Commands](./commands.md) for the command lists. In macros, wrap modified
and special keys in angle brackets, such as `@miw<A-o>`. A macro must be the
only item in a binding; it cannot be combined with other commands in a list.

Open `config.toml` with `:config-open` and add your bindings. Modifier prefixes
use `C-` for Ctrl, `A-` for Alt, and `S-` for Shift. For example:

```toml
[keys.normal]
C-s = ":write"
ret = ["open_below", "normal_mode"]
A-x = "@x<A-d>" # Select a line and delete it without yanking.

# Replace Space f with a labeled menu.
[keys.normal.space.f]
label = "File"
f = "file_picker"
s = { label = "Save", command = ":write" }
c = { label = "Edit config", command = ":config-open" }

[keys.insert]
j = { k = "normal_mode" } # Press j, then k, to leave insert mode.
```

Debug mode inherits normal bindings after user and workspace configuration is
merged. Debug defaults override those bindings, then `[keys.debug]` overrides
apply. To enter debug mode with a custom binding:

```toml
[keys.normal.space]
D = "debug_mode"

[keys.debug]
F10 = "dap_next"
```

`Esc` returns to normal mode. Entering or leaving debug mode does not start or
stop a debug session. Debug mode uses normal cursor settings and stays active
when changing buffers or splits. Insert and select commands enter their own
modes; leaving them returns to normal mode.

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

Bindings merge with the defaults. Trusted project settings override your
user settings. `:config-reload` also updates components that are already open.

Use `no_op` to consume a key without doing anything. Use `unbound` to remove
its action and let the usual fallback handle it. For example, a key unbound in
`keys.picker` can edit the picker's query through `keys.prompt`.

Nested maps wait for another key. `Esc` or an invalid continuation cancels the
sequence and consumes that key. Pasting into a prompt cancels a pending sequence
and inserts the pasted text. Action lists run in order and stop when an action
closes the component or starts another interaction.

Picker actions take priority over prompt actions. Menu actions take priority
over popup actions. To pass `Ctrl-d` through a completion menu and its popup to
an editor binding, set `C-d = "unbound"` in both `keys.menu` and `keys.popup`.
To remap `Esc` inside an insert-mode menu, also set `esc = "unbound"` in
`keys.popup` so the popup passes it to the menu.

Component bindings accept only the actions below. Editor commands and macros
belong in `keys.normal`, `keys.insert`, and `keys.select`.

| Table | Actions |
| --- | --- |
| `keys.prompt` | `accept`, `cancel`, `move_left`, `move_right`, `move_word_left`, `move_word_right`, `move_start`, `move_end`, `delete_backward`, `delete_forward`, `delete_word_backward`, `delete_word_forward`, `delete_to_start`, `delete_to_end`, `insert_word`, `insert_register`, `history_previous`, `history_next`, `complete_previous`, `complete_next`, `exit_selection` |
| `keys.picker` | `accept`, `accept_keep_open`, `accept_horizontal_split`, `accept_vertical_split`, `cancel`, `move_up`, `move_down`, `move_start`, `move_end`, `page_up`, `page_down`, `toggle_preview`, `populate_quicklist` |
| `keys.buffer-picker` | All `keys.picker` actions, plus `close_buffer` |
| `keys.quicklist-picker` | All `keys.picker` actions, plus `remove_entry` |
| `keys.menu` | `accept`, `cancel`, `move_up`, `move_down`, `page_up`, `page_down` |
| `keys.popup` | `cancel`, `scroll_up`, `scroll_down` |
| `keys.hover`, `keys.signature-help` | `previous`, `next` |
| `keys.blame` | `copy_commit_id`, `copy_message` |

All component tables also accept `no_op` and `unbound`. Popup scrolling and menu
paging move by half a page. Picker paging moves by a full page.

`keys.buffer-picker` and `keys.quicklist-picker` inherit `keys.picker`.
Picker-specific bindings take priority over shared picker bindings, including
ones from project configuration. The quicklist picker also binds `Ctrl-x` to
`remove_entry` by default.

`close_buffer` closes the selected buffer while keeping the picker open. It
waits for pending saves and refuses to discard unsaved changes. After closing,
the query is kept and a nearby row becomes selected. The action stops the
current action list. It has no default binding.

`remove_entry` removes the highlighted quicklist entry while keeping the picker
open, preserving the filter and selecting a nearby row. It stops the current
action list. See [Quicklist](./quicklist.md) for adding and removing locations.

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

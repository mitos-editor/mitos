# Keymap

These tables list the built-in bindings. To change them, see
[Key remapping](./remapping.md).

`<space>` means Space, `<ret>` means Enter, `<esc>` means Escape,
`<C-x>` means Ctrl-x, and `<A-x>` means Alt-x. Adjacent keys form a
sequence: `<space>f` means press Space, then f.
Some terminals intercept keys before Mitos receives them; check your terminal
settings if a binding does not work.

Language-server commands require an active server, and syntax-aware commands
require a tree-sitter grammar and the relevant queries for the file type.

## Normal mode

Normal mode is the default when Mitos starts. Press Escape to return to it.
Type a number before a command to give it a count, for example `42G` to go
to line 42. Unlike Vim, `f`, `F`, `t`, and `T` can cross line boundaries.

<a id="selection-manipulation"></a>

See [Selections](./usage.md) for building and transforming selections,
[Search and replacement](./search.md) for search, and
[Shell commands](./shell.md) for filtering text. Search commands use the `/`
register by default; select another register with `"<char>`.

### Minor modes

Prefix keys open groups of commands. Most return to normal mode after a
command; sticky groups stay open until Escape. See
[remapping minor modes](./remapping.md#minor-modes).

<a id="view-mode"></a>
<a id="goto-mode"></a>
<a id="match-mode"></a>
<a id="window-mode"></a>
<a id="space-mode"></a>
<a id="unimpaired"></a>

- `z` scrolls and positions the view; `Z` opens its sticky variant.
- `g` jumps to locations.
- `m` works with [surround](./surround.md) and [textobjects](./textobjects.md).
- `Ctrl-w` manages [split views](./files.md).
- Space opens pickers and project tools. `Space '` reopens global search.
- `[` and `]` move to previous and next items, including syntax textobjects.

### Bindings

{{#include ./generated/keymap-normal.md}}

## Insert mode

Press `i` in normal mode to enter insert mode. An insert session normally
forms one undo step when you return to normal mode. `Ctrl-s` starts another
undo step without leaving insert mode; see [Macros and undo](./macros.md#undo-and-redo).

Tab uses [smart tab](./completion.md#smart-tab). To disable a binding, map
it to `no_op` in `[keys.insert]`.

{{#include ./generated/keymap-insert.md}}

## Select / extend mode

Press `v` in normal mode to enter select mode. Motions extend selections
rather than replace them; `vgl`, for example, extends to the end of the line.
Searching with `n` or `N` keeps the current selection and adds the next match.

{{#include ./generated/keymap-select.md}}

## Component bindings

The tables below use the action names accepted by
[component key remapping](./remapping.md#component-keys).
[Prompt](#prompt) bindings also work in [pickers](./pickers.md), except where
they conflict with picker bindings. Buffer pickers use their own binding table.
Popups use the popup bindings together with their specific hover,
signature-help, or blame bindings. In the completion menu, other typing
accepts the completion and continues inserting text.

{{#include ./generated/keymap-components.md}}

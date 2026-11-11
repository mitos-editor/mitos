# Syntax-aware motions

## Moving the selection with syntax-aware motions

Use syntax-aware motions to select expressions and code blocks without
counting characters:

| Key | Action |
| --- | --- |
| `Alt-o` / `Alt-Up` | Expand to the enclosing structure |
| `Alt-i` / `Alt-Down` | Shrink the selection |
| `Alt-n` / `Alt-Right` | Select the next sibling |
| `Alt-p` / `Alt-Left` | Select the previous sibling |
| `Alt-a` | Select all siblings |
| `Alt-I` | Select all children |

For example, select `arg1` in `func(arg1, arg2, arg3)`. Sibling motions include
punctuation: `Alt-n` selects the comma, and pressing it again selects `arg2`.
`Alt-o` expands to the argument list. You can then copy, replace, or surround
the selected structure using the usual editing commands.

`Alt-a` and `Alt-I` create separate selections for sibling or child structures.
In an argument list or array, these commands select the values without commas
or brackets. Expansion history lets `Alt-i` retrace recent `Alt-o` steps. After
another selection command or an edit clears that history, shrinking selects a
child instead.

These motions need a syntax grammar for the current language. Use
[textobjects](./textobjects.md) when you want a specific kind of structure,
such as a function or parameter.
For worked examples of functions, siblings, parents, and children, see
[Edit code with textobjects](./editing-effectively/textobjects.md#move-between-siblings).

## How structures relate

The syntax tree explains which selections are parents and siblings. For
example:

```js
func(arg1, arg2, arg3);
```

A function call might be parsed by tree-sitter into a tree like the following.
This simplified view leaves out punctuation nodes such as parentheses and commas.

```tsq
(call
  function: (identifier) ; func
  arguments:
    (arguments           ; (arg1, arg2, arg3)
      (identifier)       ; arg1
      (identifier)       ; arg2
      (identifier)))     ; arg3
```

Use `:tree-sitter-subtree` to view the syntax tree of the main selection. In
a more intuitive tree format:

```text
            ┌────┐
            │call│
      ┌─────┴────┴─────┐
      │                │
┌─────▼────┐      ┌────▼────┐
│identifier│      │arguments│
│  "func"  │ ┌────┴───┬─────┴───┐
└──────────┘ │        │         │
             │        │         │
   ┌─────────▼┐  ┌────▼─────┐  ┌▼─────────┐
   │identifier│  │identifier│  │identifier│
   │  "arg1"  │  │  "arg2"  │  │  "arg3"  │
   └──────────┘  └──────────┘  └──────────┘
```

If you have a selection that wraps `arg1` (see the tree above), `Alt-n` first
selects the comma. Press it a second time to select `arg2`:

```js
// before
func([arg1], arg2, arg3)
// after
func(arg1, [arg2], arg3);
```

Similarly, `Alt-o` will expand the selection to the parent node, in this case, the
arguments node.

```js
func[(arg1, arg2, arg3)];
```

When there is no sibling in the requested direction, the motion moves up the
tree until it finds one. From `arg1`, `Alt-p` first selects the opening
parenthesis. Pressing `Alt-p` again moves out of the argument list and selects
`func`. The selected pieces depend on the language grammar; inspect the
highlight before applying an edit.

[lang-support]: ./lang-support.md

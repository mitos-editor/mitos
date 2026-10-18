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

For example, select `arg1` in `func(arg1, arg2, arg3)`. `Alt-n` selects
`arg2`; `Alt-o` expands to the argument list. You can then copy, replace, or
surround the selected structure using the usual editing commands.

These motions need a syntax grammar for the current language. Use
[textobjects](./textobjects.md) when you want a specific kind of structure,
such as a function or parameter.

## How structures relate

The syntax tree explains which selections are parents and siblings. For
example:

```js
func(arg1, arg2, arg3);
```

A function call might be parsed by tree-sitter into a tree like the following.

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

If you have a selection that wraps `arg1` (see the tree above), and you use
`Alt-n`, it will select the next sibling in the syntax tree: `arg2`.

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

There is also some nuanced behavior that prevents you from getting stuck on a
node with no sibling. When using `Alt-p` with a selection on `arg1`, the previous
child node will be selected. In the event that `arg1` does not have a previous
sibling, the selection will move up the syntax tree and select the previous
element. As a result, using `Alt-p` with a selection on `arg1` will move the
selection to the "func" `identifier`.

[lang-support]: ./lang-support.md

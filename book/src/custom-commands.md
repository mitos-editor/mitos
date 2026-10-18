# Custom commands

Custom commands define short command-mode (`:`) commands in `config.toml`. A custom command can run one or more built-in typable or static commands, or a single key macro.

```toml
[commands]
":wq" = [":write", ":quit"]
":f" = ":format"
":W" = ":write!"
":hints" = ":toggle lsp.inlay-hints.enable"
```

Typable commands start with `:`. Static commands, such as `move_char_right`, use their bare names. Key macros start with `@`, for example `@miw`. A macro must be the only item in a custom command.

The available typable and static commands are listed in the [Commands](./commands.md) and [Keymap](./keymap.md) documentation.

Mapped typable commands must be built-ins; custom commands cannot invoke other custom commands.

## Arguments

Use `%arg{n}` to pass a positional argument to a mapped typable command. Argument indexes start at zero.

```toml
[commands]
":line" = ":goto %arg{0}"
```

Running `:line 42` executes `:goto 42`. Missing arguments expand to an empty
string. See [Command mode syntax](./command-line.md#expansions) for other expansions.

## Descriptions and completion

Use the detailed form to document accepted arguments and inherit completion from a built-in command:

```toml
[commands.":save-as"]
commands = [':write "%arg{0}"']
desc = "Save the current buffer to a path"
accepts = "<path>"
completer = ":write"
```

The description supports Markdown and is shown in the command prompt.
Here `:save-as` uses `:write`'s path completion. For example,
`:save-as "meeting notes.txt"` saves to a path containing a space.

## Shadowing built-ins

Custom commands take precedence over built-ins. Prefix a command with `^` to bypass a custom definition:

```toml
[commands]
":w" = ":write!"
```

With this configuration, `:w` force-writes while `:^w` retains the built-in `:write` behavior.

## Hidden commands

Commands whose configuration key starts with `:` appear in command-mode completion and documentation. Omit that prefix to keep a command usable but hidden:

```toml
[commands]
"0" = ":goto 1"
```

The command remains available as `:0` but does not appear in the command list.

Global and workspace command tables are merged in the same way as the rest of the configuration. A workspace definition overrides a global command with the same name, and workspace commands are only loaded for trusted workspaces.

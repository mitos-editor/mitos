# Custom commands

Custom commands define short command-mode (`:`) commands in `config.toml`. A custom command can run one or more built-in typable or static commands, or a single key macro.

```toml
[commands]
":wq" = [":write", ":quit"]
":f" = ":format"
":W" = ":write!"
":hints" = ":toggle lsp.inlay-hints.enable"
```

Typable commands start with `:`. Static commands, such as `move_char_right`, use their bare names. Key macros start with `@`, for example `@miw`. A macro must be the only item in a custom command because command sequences and macros use different execution models.

The available typable and static commands are listed in the [Commands](./commands.md) and [Keymap](./keymap.md) documentation.

Mapped typable commands must be built-ins; custom commands cannot invoke other custom commands.

## Arguments

Use `%arg{n}` to pass a positional argument to a mapped typable command. Argument indexes start at zero.

```toml
[commands]
":case" = ":pipe xargs ccase --to %arg{0}"
```

Running `:case snake` executes `:pipe xargs ccase --to snake`. Missing arguments expand to an empty string. See [Command line](./command-line.md#expansions) for the general expansion syntax.

## Descriptions and completion

Use the detailed form to document accepted arguments and inherit completion from a built-in command:

```toml
[commands.":wcd!"]
commands = [":write! %arg{0}", ":cd %sh{ %arg{0} | path dirname }"]
desc = "Force save the buffer, then change to its directory"
accepts = "<path>"
completer = ":write"
```

The description supports Markdown and is shown in the command prompt. Here `:wcd!` also uses `:write`'s path completion.

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

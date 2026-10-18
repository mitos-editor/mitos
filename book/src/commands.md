# Commands

The generated keybinding tables use `<space>` for Space, `<ret>` for Enter,
and `<C-x>` for Ctrl-x. Adjacent keys form a sequence: `<space>f` means
press Space, then f.

## Typable commands

Press `:` to run these commands. Some accept arguments; see
[Command mode syntax](./command-line.md) for quoting, flags, and expansions.

{{#include ./generated/typable-cmd.md}}

## Static Commands

Static commands take no arguments. Run them from the command palette (`Space ?`)
or use their names in [keybindings](./remapping.md).

{{#include ./generated/static-cmd.md}}

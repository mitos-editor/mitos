# Registers

In Mitos, registers are storage locations for text and other data, such as the
result of a search. Registers can be used to cut, copy, and paste text, similar
to the clipboard in other text editors. Usage is similar to Vim, with `"` being
used to select a register.

## User-defined registers

Mitos allows you to create your own named registers for storing text, for
example:

- `"ay` - Yank the current selection to register `a`.
- `"op` - Paste the text in register `o` after the selection.

If a register is selected before invoking a change or delete command, the selection will be stored in the register and the action will be carried out:

- `"hc` - Store the selection in register `h` and then change it (delete and enter insert mode).
- `"md` - Store the selection in register `m` and delete it.

## Default registers

Commands that use registers, like yank (`y`), use a default register if none is specified.
These registers are used as defaults:

| Register character | Contains              |
| ---                | ---                   |
| `/`                | Last search           |
| `:`                | Last executed command |
| `"`                | Last yanked text      |
| `@`                | Last recorded macro   |

## Special registers

Some registers have special behavior when read from and written to.

| Register character | When read              | When written             |
| ---                | ---                    | ---                      |
| `_`                | No values are returned | All values are discarded |
| `#`                | Selection indices (first selection is `1`, second is `2`, etc.) | This register is not writable |
| `.`                | Contents of the current selections | This register is not writable |
| `%`                | Name of the current file | This register is not writable |
| `+`                | Reads from the system clipboard | Joins and yanks to the system clipboard |
| `*`                | Reads from the primary clipboard | Joins and yanks to the primary clipboard |

When yanking multiple selections to the clipboard registers, the selections
are joined with newlines. Pasting from these registers will paste multiple
selections if the clipboard was last yanked to by the Mitos session. Otherwise
the clipboard contents are pasted as one selection.

## System clipboard

Ordinary `y`, `p`, and `P` use Mitos's yank register by default. To copy and
paste between Mitos and other applications:

| Keys | Action |
| --- | --- |
| `Space y` | Copy all selections to the system clipboard |
| `Space Y` | Copy only the primary selection |
| `Space p` / `Space P` | Paste after / before each selection |
| `Space R` | Replace selections with clipboard contents |

You can also use the `+` register directly: `"+y` copies and `"+p` pastes.
To make ordinary yank and paste use the system clipboard:

```toml
[editor.clipboard]
default-yank-register = "+"
```

### Clipboard providers

Mitos detects a clipboard provider automatically. Run `ms --health clipboard`
or `:show-clipboard-provider` to see which one it uses. If copying fails,
check that the selected provider's tool is installed and available in the
session where Mitos runs.

For a remote session, OSC 52 can copy through a supporting terminal:

```toml
[editor.clipboard]
provider = "termcode"
```

Your terminal and multiplexer must allow OSC 52. Reading the clipboard may be
unavailable even when copying works; in that case use your terminal's paste
shortcut to insert clipboard text.

Custom providers run one command for copying and another for pasting. The
copy command receives text on stdin; the paste command writes text to stdout.
For example, on macOS:

```toml
[editor.clipboard.provider.custom]
yank = { command = "pbcopy" }
paste = { command = "pbpaste" }
```

See the [provider reference](./configuration.md#editorclipboardprovider) for
available providers and optional primary-selection commands.

Set `editor.pad-selection-index-register = true` to zero-pad selection numbers from the `#` register to the width of the selection count, for example `01` through `11`.

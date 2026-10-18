# Coming from Helix

Mitos is a fork of Helix and keeps its selection-first editing model. Most
movement, selection, register, and window bindings will be familiar. Check the
[keymap](./keymap.md) for Mitos's defaults.

## Configuration and runtime

Run `ms` instead of `hx`. User configuration lives in `~/.config/mitos` on
Linux and macOS, or `%AppData%\mitos` on Windows. Project settings use `.mitos/`.
Mitos does not automatically load files from Helix's configuration directory.

Copy settings you want to keep into Mitos's configuration, then run
`:config-reload`. Mitos accepts many older editor setting names and provides
[migration commands](./configuration-migration.md) to update them. Unknown
editor settings are rejected, so check errors rather than assuming every Helix
option has an equivalent. `languages.toml` keeps its existing setting names.

Keep Mitos's bundled runtime with its executable. Do not point `MITOS_RUNTIME`
at a Helix runtime; themes and language queries can differ. The default themes
are Modus Operandi and Modus Vivendi. See [Themes](./themes.md) to choose another.

## Useful differences

- [Workspace trust](./workspace-trust.md) controls project-local configuration.
- `Alt-r` opens a replacement prompt; `r` still replaces individual characters.
  Custom keymaps using `replace` for character replacement should use
  `replace_char` instead.
- The [quicklist](./quicklist.md) keeps picker locations available after closing
  the picker.
- [Spell checking](./spell-checking.md) works without a language server.
- Supported [images](./image-previews.md) open as read-only previews.
- [Custom commands](./custom-commands.md) define your own `:` commands.
- [Component keys](./remapping.md#component-keys) let you remap pickers, prompts,
  menus, and popups as well as editor modes.

Use [Files, buffers, and splits](./files.md) and
[Language servers](./lsp.md) to set up your day-to-day workflow.

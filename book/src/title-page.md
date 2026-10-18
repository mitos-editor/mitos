# Mitos

Mitos is a terminal text editor built around selections. Select the text you
want to change, then apply an action. The same commands work on one selection
or many.

[Install Mitos](./install.md) · [Learn the basics](./basics.md) ·
[View the source](https://github.com/mitos-editor/mitos)

<img class="editor-screenshot" src="./screenshots/autocomplete.png" alt="Mitos editing Rust code with completion suggestions and documentation" width="2304" height="1682">

## Get started

After installing, run the interactive tutorial:

```sh
ms --tutor
```

Or follow [Basics](./basics.md) to open a file, edit text, save, and quit.
Keep the [keymap](./keymap.md) nearby while learning.

## What you can do

- Edit several matches with [multiple selections](./usage.md).
- Select code structures with [textobjects](./textobjects.md) and
  [syntax-aware motions](./syntax-aware-motions.md).
- Find files and project search results with [pickers](./pickers.md).
- Use [language servers](./lsp.md) for navigation, diagnostics, and formatting.
- Check prose with built-in [spell checking](./spell-checking.md).
- Inspect Git changes and commits with [version control](./version-control.md).
- Filter selections through [shell commands](./shell.md).
- Choose [themes](./themes.md), [keybindings](./remapping.md), and
  [editor preferences](./editor.md).

## About Mitos

Mitos is an independent fork of [Helix](https://helix-editor.com/), written in
Rust and licensed under the Mozilla Public License 2.0. It is not endorsed by
or affiliated with the Helix project or its maintainers. See
[Coming from Helix](./from-helix.md) if you already use Helix.

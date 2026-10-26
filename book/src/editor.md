# Configure Mitos

Open your user configuration with `:config-open`. The file is
`~/.config/mitos/config.toml` on Linux and macOS, or
`%AppData%\mitos\config.toml` on Windows. On Unix, `XDG_CONFIG_HOME` can change
the configuration directory.

Start with a few preferences:

```toml
theme = "onedark"

[editor]
line-number = "relative"

[editor.cursor.shape]
insert = "bar"

[keys.normal]
C-s = ":write"
```

Save the file and run `:config-reload`. Unknown editor settings and invalid
values are reported when the configuration loads. To try another file, start
with `ms --config path/to/config.toml`.

Settings changed with `:set` or `:toggle` apply to the running editor. Add them
to `config.toml` to keep them for later sessions. For example,
`:toggle soft-wrap.enable` temporarily toggles line wrapping.

<span id="editor-section"></span>

## Configuration files

| File | Purpose |
| --- | --- |
| `config.toml` | Editor preferences, themes, keybindings, and custom commands |
| `languages.toml` | Language servers, formatters, and language-specific settings |
| `.editorconfig` | Shared conventions for matching file paths |

A project's `.mitos/config.toml` and `.mitos/languages.toml` override your user
settings after you [trust the workspace](./workspace-trust.md). Tables merge;
ordinary arrays replace earlier values. See the
[configuration reference](./configuration.md#configuration-files-and-precedence)
for precedence and [EditorConfig](./editorconfig.md) for file conventions.

Older setting names are accepted without rewriting files. See
[Configuration migration](./configuration-migration.md) to update them.

<span id="editorcursorshape-section"></span>

## Cursor and line numbers

Use `line-number = "relative"` under `[editor]` to show distances from the
current line. Insert mode and unfocused views still use absolute numbers.
Choose `block`, `bar`, or `underline` for each mode's cursor:

```toml
[editor.cursor.shape]
normal = "block"
insert = "bar"
select = "underline"
```

Only the primary cursor can change shape in a terminal.

To use the terminal's block cursor and cursor colors, set
`block-rendering = "native"` under `[editor.cursor]`. The default, `"software"`,
uses the editor theme. Secondary cursors remain software blocks.

<span id="editorstatusline-section"></span>

## Statusline

Each view has its own statusline. To use one statusline for the focused view,
add `global = true` under `[editor.statusline]`. Use
`:toggle statusline.global` to try it temporarily.

Choose which elements to show on each side:

```toml
[editor.statusline]
left = ["mode", "file-name", "file-modification-indicator"]
right = ["diagnostics", "position", "file-type"]
mode.normal = "NORMAL"
mode.insert = "INSERT"
mode.select = "SELECT"
```

The [statusline reference](./configuration.md#editorstatusline) lists all
available elements and defaults.

<span id="editorbreadcrumb-section"></span>

## Breadcrumbs

Show the file path and enclosing symbols above the text:

```toml
[editor.breadcrumb]
enable = true
path = "full"
```

Use `path = "file"` to show only the filename before the symbols, or
`path = "none"` to show symbols without a path.

<span id="editorsoft-wrap-section"></span>

## Line wrapping

Soft wrapping displays a long line on several screen lines without changing
its contents:

```toml
[editor.soft-wrap]
enable = true
wrap-indicator = "↪ "
```

Set `wrap-at-text-width = true` to wrap at `editor.text-width` when that is
narrower than the view. Use `:reflow` to insert actual line breaks into selected
prose instead. Languages can override soft wrapping and text width.

<span id="editorwhitespace-section"></span>

## Visible whitespace

By default, whitespace is shown only at the end of document lines, including
the final line without a newline. This is equivalent to:

```toml
[editor.whitespace]
render = "trailing"
```

Character glyphs keep their defaults unless configured separately. Whitespace
at a soft wrap or viewport edge is only shown if it also belongs to the trailing
whitespace of that document line.

To show all whitespace characters:

```toml
[editor.whitespace]
render = "all"
```

Alternatively, choose which kinds to show:

```toml
[editor.whitespace.render]
space = "none"
tab = "all"
newline = "all"

[editor.whitespace.characters]
tab = "→"
tabpad = "·"
tabend = "←" # Optional final tab cell; defaults to tabpad
```

Each kind also accepts `"trailing"` independently, for example:

```toml
[editor.whitespace.render]
space = "trailing"
tab = "all"
newline = "none"
```

Newline markers use `"all"` or `"none"`; `"trailing"` leaves them hidden. The
`ui.virtual.trailing_whitespace` theme scope styles trailing whitespace and falls
back to `ui.virtual.whitespace`. Virtual text and wrap indicators do not count as
trailing whitespace.

Use `:set whitespace.render all` or `:set whitespace.render trailing` to change
whitespace visibility temporarily.

<span id="editorindent-guides-section"></span>

## Indentation guides

```toml
[editor.indent-guides]
render = true
character = "│"
skip-levels = 1
```

`skip-levels` hides the first indentation levels. See
[EditorConfig](./editorconfig.md) for shared indentation conventions.

## More preferences

| Feature | Guide |
| --- | --- |
| <span id="editorclipboard-section"></span>Clipboard | [Registers and clipboard](./registers.md#system-clipboard) |
| <span id="editorlsp-section"></span><span id="editordiagnosticsinline-section"></span>Diagnostics and language tools | [Language servers](./lsp.md) |
| <span id="terminallauncher-section"></span>Debugging terminal | [Debugging](./debugging.md#terminal-for-the-debugged-program) |
| <span id="editorinline-blame-section"></span><span id="format"></span>Inline Git blame | [Version control](./version-control.md#inline-blame) |
| <span id="editordiff-section"></span>Diff comparison engine | [Syntax-aware review](./version-control.md#syntax-aware-review) |
| <span id="editorfile-picker-section"></span><span id="editorfile-explorer-section"></span>File navigation and ignore rules | [Pickers](./pickers.md#hidden-and-ignored-files) |
| <span id="editorpicker-section"></span>Picker layout | [Picker layout](./pickers.md#layout) |
| <span id="editorbuffer-picker-section"></span>Buffer picker | [Initial selection](./configuration.md#editorbuffer-picker) |
| <span id="editorauto-pairs-section"></span>Delimiter pairing | [Auto-pairs](./configuration.md#editorauto-pairs) |
| <span id="editorfilesauto-save-section"></span>Saving and reloading | [Files, buffers, and splits](./files.md#changes-made-outside-mitos) |
| <span id="editorsearch-section"></span>Search | [Search and replacement](./search.md) |
| <span id="editorgutters-section"></span><span id="editorguttersline-numbers-section"></span><span id="editorguttersdiagnostics-section"></span><span id="editorguttersdiff-section"></span><span id="editorguttersspacer-section"></span><span id="editorgutterscode-action-hint-section"></span>Gutters | [Gutter layout](./configuration.md#editorgutters) |
| <span id="editorindentsmart-tab-section"></span><span id="editorcompletionword-section"></span>Completion and Tab | [Completion and snippets](./completion.md) |
| <span id="editorspelling-section"></span>Spelling | [Spell checking](./spell-checking.md) |
| <span id="editorworkspace-trust-section"></span>Project settings | [Workspace trust](./workspace-trust.md) |

See the [configuration reference](./configuration.md) for all accepted settings,
[Key remapping](./remapping.md) for bindings, and [Themes](./themes.md) for colors.

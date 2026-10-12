# Configuration

This page is the complete reference for user configuration. It lists every
setting accepted by `config.toml` and `languages.toml`; the linked chapters
explain the larger features and show more examples.

## Configuration files and precedence

The user configuration directory is:

- Linux and macOS: `~/.config/mitos`
- Windows: `%AppData%\mitos`

The main file is `config.toml`. Open it from Mitos with `:config-open`, then
apply changes with `:config-reload`. On Unix, sending `USR1` to the Mitos
process also reloads it, for example with `pkill -USR1 ms`.

Pass `-c <path>` or `--config <path>` to use another `config.toml`:

```sh
ms --config path/to/custom-config.toml
```

A project may override both files with `.mitos/config.toml` and
`.mitos/languages.toml`. Project values are merged over user values, which are
merged over built-in defaults. Project files are loaded only after the
workspace is trusted; see [Workspace trust](./workspace-trust.md).

> [!IMPORTANT]
> `[editor.workspace-trust]` is always read from the user `config.toml`.
> A project cannot weaken the trust policy that decides whether its own files
> may be loaded.

Unknown `config.toml` editor keys are rejected. This makes misspelled settings
fail visibly instead of being silently ignored.

## Structure and compatibility

Settings are grouped by feature. For example, use `editor.lsp.messages.enable`
and `editor.completion.trigger-delay`. Feature tables can be written explicitly
or with TOML dotted keys. Editor durations are milliseconds; language-server
request timeouts are seconds.

Legacy paths remain accepted, including `editor.lsp.display-messages`,
`editor.auto-format`, and the old boolean or string feature shorthands. Each
file is normalized before layering, so a project override wins regardless of
which spelling is used. Defining the same logical setting with both spellings
in one file is an error. Tables merge recursively and ordinary arrays replace
the earlier value, including an explicit empty array.

Runtime commands use editor-relative paths, for example
`:toggle lsp.inlay-hints.enable` and `:set completion.trigger-delay 100`.
Terminal settings use `terminal.*`, such as `:toggle terminal.true-color`.
Legacy command paths remain accepted; completions show canonical paths.

## Configuration migration

Mitos accepts known legacy settings and converts them in memory before merging
configuration layers. Startup and `:config-reload` leave configuration files
unchanged, including files managed by Home Manager or other dotfile managers.

To update a file to the current feature sections, run:

- `:config-migrate` for your user `config.toml`, or the file selected with `-c` or
  `--config` for this session.
- `:config-migrate-workspace` for the current trusted project's `.mitos/config.toml`.

Each command opens a confirmation dialog naming the file. **Cancel** is selected
initially; choose **Migrate** to rewrite it. Escape also cancels. User and workspace
migration are separate actions. Project migration changes the project's working
tree. Untrusted, stale, or excluded project configuration cannot be migrated.

Migration preserves values, comments, keybindings, and custom commands. All loaded
configuration layers must validate before any rewrite. Each changed file receives
an adjacent backup named `config.toml.bak`; existing backups are preserved with
numbered suffixes such as `config.toml.bak.1`. Replacements are atomic and preserve
file permissions and symlinks. For symlinks, the target file is updated and the
backup is placed beside that target. The dialog shows the target, and a status
message reports the updated file and backup path. Save or discard unsaved edits to
an open configuration buffer before migrating.

Already migrated files are left unchanged, and missing files are not created.
Read-only files are not rewritten. If a backup or rewrite fails, the loaded
settings remain usable through compatibility aliases.

For generated or read-only configuration, export the migrated TOML:

```sh
ms --migrate-config ~/.config/mitos/config.toml --stdout > /tmp/mitos-config.toml
```

This command reads only the specified file, prints its migrated contents (or the
original contents if no migration is needed), and creates no backups. Inspect the
output and update the source managed by Home Manager or your dotfile manager.
Export to a different file; redirecting stdout to the input file would truncate it.

An existing explicit workspace trust grant is updated only when the migration and
its backup explain every change to the trusted configuration fingerprint. Other
changes still require review and renewed trust; migration never grants trust to
an untrusted project. `languages.toml` is unchanged.

## `config.toml` reference

`config.toml` accepts six top-level entries:

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `theme` | string or table | `{ light = "modus_operandi", dark = "modus_vivendi" }` | Theme name, or `{ light = "...", dark = "...", fallback = "..." }`. `fallback` is optional and defaults to `dark`. See [Themes](./themes.md). |
| `editor` | table | See below | Shared editor behavior and UI settings. |
| `terminal` | table | See below | Terminal capabilities, keyboard protocol, and external launcher. |
| `keys` | table | Built-in keymap | Per-mode key bindings under `[keys.normal]`, `[keys.insert]`, and `[keys.select]`. Entries are merged with the built-in keymap. See [Key remapping](./remapping.md). |
| `commands` | table | `{}` | User-defined command-mode commands. See [Custom commands](./custom-commands.md). |
| `plugins` | table | `{}` | Named WASM plugins, their manifest paths and configuration. See [Plugins](./plugins.md). |

Example:

```toml
theme = "onedark"

[editor]
line-number = "relative"
mouse.enable = false
icons = true # Requires a Nerd Font.

[editor.cursor.shape]
insert = "bar"
normal = "block"
select = "underline"

[keys.normal]
C-s = ":write"

[commands]
":wq" = [":write", ":quit"]
```

### `[editor]`

For examples and behavioral details, see the [Editor](./editor.md) chapter.

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `welcome-screen` | boolean | `true` | Show the welcome screen when Mitos starts without a file. |
| `shell` | array of strings | Unix: `["sh", "-c"]`; Windows: `["cmd", "/C"]` | Command prefix used to execute shell commands. |
| `line-number` | `"absolute"` or `"relative"` | `"absolute"` | Line-number display mode. Relative numbers become absolute in insert mode and when the view is unfocused. |
| `gutters` | array or table | `["diagnostics", "spacer", "line-numbers", "spacer", "diff"]` | Gutter layout and options. See [`[editor.gutters]`](#editorgutters). |
| `auto-pairs` | table | `{ enable = true }` | Insert matching delimiters. See [`[editor.auto-pairs]`](#editorauto-pairs). |
| `spelling` | table | Disabled | Dictionaries, accepted words, and token filters. See [`[editor.spelling]`](#editorspelling). |
| `text-width` | integer | `80` | Width used by `:reflow` and optionally soft wrapping. |
| `idle-timeout` | milliseconds | `250` | Idle delay used by editor UI timers. |
| `continue-comments` | boolean | `true` | Continue a line comment when Enter creates a new line inside it. |
| `auto-info` | boolean | `true` | Display contextual information popups. |
| `icons` | boolean | `false` | Show UI icons. The glyphs require a Nerd Font. |
| `file-picker` | table | See below | File-picker and global-search traversal. See [`[editor.file-picker]`](#editorfile-picker). |
| `file-explorer` | table | See below | File-explorer traversal. See [`[editor.file-explorer]`](#editorfile-explorer). |
| `buffer-picker` | table | See below | Initial selection in the buffer picker. See [`[editor.buffer-picker]`](#editorbuffer-picker). |
| `statusline` | table | See below | Statusline layout and labels. See [`[editor.statusline]`](#editorstatusline). |
| `search` | table | See below | Search matching and wrap behavior. See [`[editor.search]`](#editorsearch). |
| `lsp` | table | See below | Global LSP behavior. See [`[editor.lsp]`](#editorlsp). |
| `rulers` | array of integers | `[]` | Columns at which to draw rulers; a language may override this. |
| `whitespace` | table | See below | Visible whitespace rendering. See [`[editor.whitespace]`](#editorwhitespace). |
| `breadcrumb` | table | See below | Breadcrumb navigation bar. See [`[editor.breadcrumb]`](#editorbreadcrumb). |
| `indent-guides` | table | See below | Vertical indentation guides. See [`[editor.indent-guides]`](#editorindent-guides). |
| `soft-wrap` | table | See below | Soft wrapping. See [`[editor.soft-wrap]`](#editorsoft-wrap). |
| `jump-label-alphabet` | string of unique characters | `"abcdefghijklmnopqrstuvwxyz"` | Alphabet used to generate two-character jump labels. Earlier characters are used first. |
| `workspace-trust` | table | See below | Implicit workspace trust policy. See [`[editor.workspace-trust]`](#editorworkspace-trust). |
| `bufferline` | table | See below | See [`[editor.bufferline]`](#editorbufferline). |
| `clipboard` | table | See below | See [`[editor.clipboard]`](#editorclipboard). |
| `completion` | table | See below | See [`[editor.completion]`](#editorcompletion). |
| `cursor` | table | See below | See [`[editor.cursor]`](#editorcursor). |
| `diagnostics` | table | See below | See [`[editor.diagnostics]`](#editordiagnostics). |
| `files` | table | See below | See [`[editor.files]`](#editorfiles). |
| `format` | table | See below | See [`[editor.format]`](#editorformat). |
| `indent` | table | See below | See [`[editor.indent]`](#editorindent). |
| `inline-completion` | table | See below | See [`[editor.inline-completion]`](#editorinline-completion). |
| `mouse` | table | See below | See [`[editor.mouse]`](#editormouse). |
| `rainbow-brackets` | table | See below | See [`[editor.rainbow-brackets]`](#editorrainbow-brackets). |
| `scroll` | table | See below | See [`[editor.scroll]`](#editorscroll). |

### `[editor.scroll]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `padding` | integer | `5` | Minimum screen lines kept above and below the cursor when possible. |
| `wheel-lines` | integer | `3` | Lines moved by one scroll-wheel step. |

### `[editor.mouse]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `enable` | boolean | `true` | Enable mouse input. |
| `yank-register` | character | `"*"` | Register used for selections made with the mouse. |
| `middle-click-paste` | boolean | `true` | Paste the primary selection on middle click. |

### `[editor.clipboard]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `default-yank-register` | character | `"\""` | Register used by yank and paste commands when none is selected explicitly. |
| `provider` | provider name or custom table | Auto-detected | Clipboard integration. See [`[editor.clipboard.provider]`](#editorclipboardprovider). |

### `[editor.cursor]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `highlight-line` | boolean | `false` | Highlight every line containing a cursor. |
| `highlight-column` | boolean | `false` | Highlight every column containing a cursor. |
| `shape` | table | All modes `"block"` | Cursor shape by mode. See [`[editor.cursor.shape]`](#editorcursorshape). |

### `[editor.completion]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `auto-trigger` | boolean | `true` | Show completion automatically. |
| `word` | table | `{ enable = true, trigger-length = 7 }` | Complete words from open buffers. See [`[editor.completion.word]`](#editorcompletionword). |
| `trigger-delay` | milliseconds | `250` | Delay after typing a word character before completion is shown; use `5` for effectively immediate completion. |
| `preview-insert` | boolean | `true` | Temporarily insert the selected completion item while browsing the menu. |
| `trigger-length` | integer | `2` | Minimum word length that triggers automatic completion. |
| `replace` | boolean | `false` | Ask LSP completions to replace the whole word instead of only the text before the cursor. |

Path completion is configured by `path.enable` (default `true`); word
completion has its own [`[editor.completion.word]`](#editorcompletionword) table.

### `[editor.completion.path]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `enable` | boolean | `true` | Complete filesystem paths recognized at the cursor. |

### `[editor.format]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `on-save` | boolean | `true` | Format on save when the current language also enables `auto-format`. |

### `[editor.files]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `auto-save` | table | Disabled | Save on focus loss and/or after a delay. See [`[editor.files.auto-save]`](#editorfilesauto-save). |
| `reload` | table | See below | Reload buffers when files change externally. See [`[editor.files.reload]`](#editorfilesreload). |
| `watch` | table | See below | Native file watching, traversal, and Git refreshes. See [`[editor.files.watch]`](#editorfileswatch). |
| `default-line-ending` | `"native"`, `"lf"`, `"crlf"`, `"ff"`, `"cr"`, or `"nel"` | `"native"` | Line ending for new documents. The last three values require the `unicode-lines` build feature. |
| `editor-config` | boolean | `true` | Apply supported settings from `.editorconfig` files. |

Saving, autosave, reloads, and native watching are configured in the
`save`, `auto-save`, `reload`, and `watch` subsections.

### `[editor.inline-completion]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `trigger-delay` | milliseconds | `150` | Delay after editing before requesting LSP ghost text. |
| `auto-trigger` | boolean | `true` | Request LSP ghost text automatically in insert mode. |

### `[editor.bufferline]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `show` | `"never"`, `"always"`, or `"multiple"` | `"never"` | When to show open buffers at the top of the editor. |

### `[editor.files.save]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `insert-final-newline` | boolean | `true` | Add a final line ending on write when missing. |
| `atomic` | boolean | `true` | Write through a temporary file and replace the original. Safer against interruption, but may confuse file watchers. |
| `trim-final-newlines` | boolean | `false` | Remove line endings after the final line ending on write. |
| `trim-trailing-whitespace` | boolean | `false` | Remove whitespace before line endings on write. |

### `[editor.indent]`

The `smart-tab` subsection controls syntax-aware Tab behavior.

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `smart-tab` | table | See below | Syntax-aware Tab behavior. See [`[editor.indent.smart-tab]`](#editorindentsmart-tab). |
| `heuristic` | `"simple"`, `"tree-sitter"`, or `"hybrid"` | `"hybrid"` | Indentation strategy. Unavailable strategies fall back from hybrid to tree-sitter to simple. |

### `[editor.diagnostics]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `inline` | table | See below | Diagnostics rendered within buffer text. See [`[editor.diagnostics.inline]`](#editordiagnosticsinline). |
| `end-of-line` | table | `{ severity = "hint" }` | End-of-line diagnostic severity. |

### `[editor.diagnostics.end-of-line]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `severity` | `"disable"`, `"hint"`, `"info"`, `"warning"`, or `"error"` | `"hint"` | Minimum severity shown at the end of a line. |

### `[editor.rainbow-brackets]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `enable` | boolean | `false` | Color matching bracket levels. The language needs a `rainbows.scm` query. |

### `[editor.breadcrumb]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `enable` | boolean | `false` | Show the breadcrumb bar. |
| `path` | `"full"`, `"file"`, or `"none"` | `"full"` | Include the full path, only the file name, or no path before symbols. |

### `[editor.clipboard.provider]`

Set `provider` to `"pasteboard"`, `"wayland"`, `"x-clip"`,
`"x-sel"`, `"win32-yank"`, `"tmux"`, `"windows"`, `"termux"`,
`"termcode"`, or `"none"`. Availability is platform- and build-dependent.
Without an explicit value, Mitos selects a usable provider from the current
platform and environment.

A custom provider uses this shape:

```toml
[editor.clipboard.provider.custom]
yank = { command = "copy-command", args = ["--clipboard"] }
paste = { command = "paste-command", args = ["--clipboard"] }
yank-primary = { command = "copy-command", args = ["--primary"] }   # optional
paste-primary = { command = "paste-command", args = ["--primary"] } # optional
```

`yank` and `paste` are required. Every command has a required `command` string
and optional `args` array (default `[]`). Mitos sends yanked text to stdin and
reads pasted text from stdout. `yank-primary` and `paste-primary` are optional;
they handle the primary selection where the platform has one.

### `[editor.statusline]`

Each view has its own statusline by default. Set `global = true` to show a
single statusline at the bottom of the editor for the focused view.

| Key | Type | Default |
| --- | --- | --- |
| `global` | boolean | `false` |
| `left` | array of element names | `["mode", "spinner", "branch", "file-name", "read-only-indicator", "file-modification-indicator"]` |
| `right` | array of element names | `["diagnostics", "selections", "register", "position", "file-encoding"]` |
| `separator` | string | `"│"` |
| `mode.color` | boolean | `false` |
| `mode.normal` | string | `"NOR"` |
| `mode.insert` | string | `"INS"` |
| `mode.select` | string | `"SEL"` |
| `diagnostics` | severity array | `["warning", "error"]` |
| `workspace-diagnostics` | severity array | `["warning", "error"]` |

Valid elements and their behavior are listed in the
[statusline section](./editor.md#editorstatusline-section).

### `[editor.lsp]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `enable` | boolean | `true` | Enable LSP integration globally. |
| `workspace-roots` | array of paths | `[]` | Workspace-relative directories that stop LSP root discovery. |
| `progress.enable` | boolean | `false` | Show `$/progress` messages below the statusline. |
| `messages.enable` | boolean | `true` | Show `window/showMessage` messages below the statusline. |
| `signature-help.auto-trigger` | boolean | `true` | Open signature help automatically. |
| `signature-help.show-documentation` | boolean | `true` | Show documentation in signature help. |
| `inlay-hints.enable` | boolean | `false` | Render inlay hints. The server may also require configuration. |
| `document-highlight.auto-trigger` | boolean | `true` | Highlight references to the symbol at the cursor. |
| `inlay-hints.length-limit` | non-zero integer or unset | Unset | Truncate displayed inlay hints to this length. |
| `color-swatches.enable` | boolean | `true` | Render swatches beside document colors. |
| `code-lenses.enable` | boolean | `true` | Show code lens labels at the end of each line, resolving visible lenses automatically. Labels use space left by diagnostics and are clipped without wrapping. Gutter indicators and `Space+l` provide access to the full commands. |
| `completion.snippets` | boolean | `true` | Advertise and insert LSP snippet completions. A server restart is required after changing it. |
| `references.include-declaration` | boolean | `true` | Include a symbol's declaration in reference results. |

The dotted paths above are feature subsections. For example:

```toml
[editor.lsp.messages]
enable = true

[editor.lsp.signature-help]
auto-trigger = true
show-documentation = false

[editor.lsp.inlay-hints]
enable = true
length-limit = 40
```

### `[editor.cursor.shape]`

`normal`, `insert`, and `select` each accept `"block"`, `"bar"`,
`"underline"`, or `"hidden"`; all default to `"block"`. Terminals can change
only the primary cursor's shape.

### `[editor.files.reload]`

Mitos reloads unmodified buffers when their files change externally, preserving
undo history and updating selections in every split. If a buffer has unsaved
changes, a prompt offers **Enter** to reload or **Esc** to keep the buffer. The
same external change prompts only once. Deleted files keep their open buffers;
recreating the file can trigger another reload.

| Key | Default | Description |
| --- | --- | --- |
| `enable` | `true` | Automatically reload files changed on disk. |
| `prompt-if-modified` | `true` | Prompt before reloading a buffer with unsaved changes. When disabled, show a warning instead. |
| `poll.enable` | `true` | Periodically check open files outside native watcher coverage and Git metadata. |
| `poll.interval` | `30000` | Polling interval in milliseconds, with a minimum of `100`. |

Files outside watched roots, hidden or ignored files, and files beyond the depth
limit are checked when the terminal regains focus and by periodic polling. This
also provides a fallback when a native watcher cannot start. Focus checks remain
active when `poll.enable` is `false`. Git refreshes use the same polling schedule
and can continue when buffer auto-reload is disabled.

```toml
[editor.files.reload]
enable = true
prompt-if-modified = true

[editor.files.reload.poll]
enable = true
interval = 30000
```

### `[editor.files.watch]`

Native file events also notify language servers registered for file creation,
changes, and deletion. Relative LSP patterns may add watch roots outside the
current workspace. Recursive watching defaults to workspaces so starting Mitos
in a home directory does not watch the entire home directory.

| Key | Default | Description |
| --- | --- | --- |
| `enable` | `true` | Enable native recursive file watching. Buffer polling and focus checks can still run when disabled. |
| `watch-vcs` | `true` | Refresh branch names and diff gutters when Git HEAD or branch references change, including linked worktrees. |
| `require-workspace` | `true` | Automatically watch the working directory only when a workspace is found. Explicit LSP roots are still allowed. |
| `hidden` | `true` | Exclude hidden paths, apart from editor/build configuration and the Git metadata needed for refreshes. |
| `ignore` | `true` | Read `.ignore` files. |
| `git-ignore` | `true` | Read `.gitignore` files, including nested files. |
| `git-global` | `true` | Read Git's global ignore file. |
| `max-depth` | `10` | Maximum path depth below a watch root. Deeper open files use polling. |

The global Mitos `ignore` file and `.mitos/ignore` also apply. For watcher-specific
rules, use `filesentryignore` in the Mitos configuration directory or
`.mitos/filesentryignore` in the workspace. These use gitignore syntax and take
priority over the other filters. Changes to workspace ignore files refresh the
watcher's filters. Configuration reloads and `:cd` update watch coverage.

```toml
[editor.files.watch]
enable = true
watch-vcs = true
require-workspace = true
hidden = true
ignore = true
git-ignore = true
git-global = true
max-depth = 10
```

### `[editor.file-picker]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `hidden` | boolean | `true` | Ignore hidden files. |
| `follow-symlinks` | boolean | `true` | Follow symbolic links. |
| `deduplicate-links` | boolean | `true` | Hide links to files already present in results. |
| `parents` | boolean | `true` | Read ignore files from parent directories. |
| `ignore` | boolean | `true` | Read `.ignore` files. |
| `git-ignore` | boolean | `true` | Read `.gitignore` files. |
| `git-global` | boolean | `true` | Read Git's global excludes file. |
| `git-exclude` | boolean | `true` | Read `.git/info/exclude`. |
| `max-depth` | integer or unset | Unset | Maximum directory recursion depth. |

### `[editor.file-explorer]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `hidden` | boolean | `false` | Ignore hidden files. |
| `follow-symlinks` | boolean | `false` | Follow symbolic links. |
| `parents` | boolean | `false` | Read ignore files from parent directories. |
| `ignore` | boolean | `false` | Read `.ignore` files. |
| `git-ignore` | boolean | `false` | Read `.gitignore` files. |
| `git-global` | boolean | `false` | Read Git's global excludes file. |
| `git-exclude` | boolean | `false` | Read `.git/info/exclude`. |

### `[editor.buffer-picker]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `start-position` | `"current"` or `"previous"` | `"current"` | Initially select the current buffer or the previously focused buffer. |

### `[editor.auto-pairs]`

`enable` defaults to `true`. Omit `pairs` to use the built-in delimiter pairs.
A `pairs` table maps each single opening character to its closing character;
an empty map enables no pairs. `enable = false` disables pairing while retaining
any configured pairs.

```toml
[editor.auto-pairs]
enable = true
pairs = { '(' = ')', '[' = ']', '{' = '}', '"' = '"' }
```

Legacy booleans and direct character maps remain accepted.

### `[editor.files.auto-save]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `focus-lost` | boolean | `false` | Save when terminal focus moves away from Mitos. |
| `after-delay.enable` | boolean | `false` | Save after no edits occur for the configured delay. |
| `after-delay.timeout` | milliseconds | `3000` | Delay used when `after-delay.enable` is true. |

The legacy shorthand `editor.auto-save = true` enables only `focus-lost`; it does not
enable delayed saving.

### `[editor.search]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `smart-case` | boolean | `true` | Search case-insensitively unless the pattern contains an uppercase character. |
| `wrap-around` | boolean | `true` | Continue at the other end of the document after the last match. |

### `[editor.whitespace]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `render` | `"none"`, `"all"`, or table | `"none"` | Render all whitespace uniformly, or configure kinds separately. |
| `render.default` | `"none"` or `"all"` | `"none"` | Fallback for omitted per-kind rendering values. |
| `render.space` | `"none"` or `"all"` | Inherits `default` | Render ordinary spaces. |
| `render.nbsp` | `"none"` or `"all"` | Inherits `default` | Render non-breaking spaces. |
| `render.nnbsp` | `"none"` or `"all"` | Inherits `default` | Render narrow non-breaking spaces. |
| `render.tab` | `"none"` or `"all"` | Inherits `default` | Render tabs. |
| `render.newline` | `"none"` or `"all"` | Inherits `default` | Render line endings. |
| `characters.space` | character | `"·"` | Glyph for a space. |
| `characters.nbsp` | character | `"⍽"` | Glyph for a non-breaking space. |
| `characters.nnbsp` | character | `"␣"` | Glyph for a narrow non-breaking space. |
| `characters.tab` | character | `"→"` | Glyph at the start of a tab. |
| `characters.tabpad` | character | `" "` | Glyph used to fill the rest of a tab. |
| `characters.newline` | character | `"⏎"` | Glyph for a line ending. |

### `[editor.indent-guides]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `render` | boolean | `false` | Render vertical indent guides. |
| `character` | character | `"│"` | Guide glyph. |
| `skip-levels` | integer | `0` | Indentation levels to omit before drawing guides. |

### `[editor.gutters]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `layout` | array | `["diagnostics", "spacer", "line-numbers", "spacer", "diff"]` | Ordered gutter components. Accepted names are `diagnostics`, `line-numbers`, `spacer`, `diff`, and `code-action-hint`. |
| `line-numbers.min-width` | integer | `3` | Minimum width of the line-number gutter. |

`diagnostics`, `diff`, `spacer`, and `code-action-hint` currently have no
component-specific settings.

### `[editor.soft-wrap]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `enable` | boolean | `false` | Wrap lines wider than the view. |
| `max-wrap` | integer | `20` | Maximum free columns used to find a word boundary. Also limited to one quarter of the viewport. |
| `max-indent-retain` | integer | `40` | Maximum indentation carried onto wrapped lines. Also limited to one quarter of the viewport. |
| `wrap-indicator` | string | `"↪ "` | Text shown before continuation lines. |
| `wrap-at-text-width` | boolean | `false` | Wrap at `editor.text-width` when it is narrower than the viewport. |

### `[editor.indent.smart-tab]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `enable` | boolean | `true` | Move to the end of the parent syntax node when non-whitespace precedes the cursor; otherwise insert indentation. |
| `supersede-menu` | boolean | `false` | Give smart-tab precedence over completion-menu navigation. |

### `[editor.diagnostics.inline]`

Severity settings accept `"disable"`, `"hint"`, `"info"`, `"warning"`, or
`"error"`.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `cursor-line` | severity | `"warning"` | Minimum severity rendered inline on the primary cursor line. Disabled in insert mode and delayed briefly after moving lines. |
| `other-lines` | severity | `"disable"` | Minimum severity rendered inline on other lines. |
| `min-diagnostic-width` | integer | `40` | Minimum columns reserved for diagnostic text; inline diagnostics are hidden if the view cannot provide this plus the prefix. |
| `prefix-len` | integer | `1` | Horizontal bars rendered before diagnostic text. |
| `max-wrap` | integer | `20` | Maximum free columns used to wrap diagnostic text. |
| `max-diagnostics` | integer | `10` | Maximum inline diagnostics rendered for one line. |

### `[editor.spelling]`

Global options for spell checking. See the [Spell
checking](./spell-checking.md) chapter for the full feature details.

| Key               | Description                                                                                              | Default |
| ---               | ---                                                                                                      | ---     |
| `languages`       | The dictionaries to check every document against (e.g. `["en_US"]`). Empty disables spell checking.      | `[]`    |
| `words`           | Extra accepted words, matched case-insensitively, in addition to the dictionaries.                       | `[]`    |
| `ignore-regexes`  | Tokens matching any of these regexes are not checked (for example `"^[A-Z0-9_]+$"` to skip `CONSTANTS`). | `[]`    |
| `min-word-length` | Tokens shorter than this are not checked.                                                                | `1`     |

Per-language settings in `languages.toml` override these: `languages` and
`min-word-length` replace the global value, while `words` and `ignore-regexes`
are added to the global lists.

Example:

```toml
[editor.spelling]
languages = ["en_US"]
words = ["Mitos", "tokio"]
ignore-regexes = ["^[A-Z0-9_]+$"]
```


### `[editor.completion.word]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `enable` | boolean | `true` | Complete words found in open buffers. |
| `trigger-length` | non-zero integer | `7` | Word length at which this completion source activates. |

### `[terminal]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `true-color` | boolean | `false` | Force true-color support when detection produces a false negative. |
| `undercurl` | boolean | `false` | Force undercurl support when detection produces a false negative. |
| `keyboard.kitty-protocol` | `"auto"`, `"enabled"`, or `"disabled"` | `"auto"` | Policy for the Kitty enhanced keyboard protocol. |
| `launcher` | table | Environment-dependent | External terminal or multiplexer command. See below. |

### `[terminal.launcher]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `command` | string | Environment-dependent | External terminal or multiplexer executable. |
| `args` | array of strings | `[]` | Arguments placed before the command Mitos asks the terminal to run. |

Mitos automatically uses a tmux split inside tmux, a WezTerm split when its
Unix socket is available, Windows Terminal when found on Windows, or Conhost as
the Windows fallback. On other terminals this setting is unset unless configured.

### `[editor.workspace-trust]`

| Key | Type / accepted values | Default | Description |
| --- | --- | --- | --- |
| `level` | `"none"`, `"servers"`, or `"insecure"` | `"servers"` | What may run without a per-workspace grant. `servers` permits LSP and DAP processes but still gates project config and repository-local Git configuration. |
| `prompt` | boolean | `true` | Show the trust modal. The restricted-workspace indicator is independent of this setting. |
| `trusted` | array of glob strings | `[]` | Trust matching workspace paths without a grant. This skips `.mitos` change detection and should be used cautiously. |

This table is user-scope only. See [Workspace trust](./workspace-trust.md) for
the security model and recommended configurations.

### `[keys]`

`[keys.normal]`, `[keys.insert]`, and `[keys.select]` are maps whose keys are
key names and whose values are a static command, a `:typable-command`, a key
macro beginning with `@`, a sequence of commands, or another table forming a
minor mode. There is no fixed list of entries: omitted bindings retain their
built-in value. See [Key remapping](./remapping.md) for key syntax and examples.

### `[commands]`

Each key defines a command-mode command. Its value is one command string, an
array of command strings, or a table with these settings:

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `commands` | array of strings | Required | Built-in typable/static commands to execute in order. A key macro may be used only by itself. |
| `desc` | string or unset | Unset | Markdown description displayed in command help. |
| `accepts` | string or unset | Unset | Argument placeholder shown in command help. |
| `completer` | built-in typable command or unset | Unset | Reuse that command's argument completion. |

See [Custom commands](./custom-commands.md) for argument expansion, visibility,
and built-in-command shadowing.

## `languages.toml` reference

`languages.toml` has three top-level entries: `[[language]]` definitions,
`[language-server.<name>]` definitions, and `[[grammar]]` sources. The optional
`use-grammars` selector affects grammar fetch/build commands. User and trusted
project entries are merged over the built-in language definitions.

Defaults below are schema fallbacks for a newly defined language. A built-in
language may already set a different effective value before user and project
overrides are merged.

See [Languages](./languages.md) for file detection, root selection, LSP feature
routing, formatters, and complete examples.

`languages.toml` retains its existing keys, including `auto-format`,
`path-completion`, and `word-completion`. The feature-table restructuring
applies to `config.toml` for now.

### `[[language]]`

| Key | Type / default | Description |
| --- | --- | --- |
| `name` | string; required | Language name and merge key. |
| `language-id` | string; defaults to `name` | Language identifier sent to language servers. |
| `scope` | string; required for a new language | TextMate-style scope such as `source.rust`. |
| `file-types` | array; required for a new language | File extensions or `{ glob = "..." }` matchers. |
| `shebangs` | string array; `[]` | Interpreter names recognized in shebangs. |
| `roots` | glob array; `[]` | Markers used to choose an LSP working directory. |
| `comment-tokens` | string or string array; unset | Line-comment tokens. `comment-token` is accepted as a compatibility alias. |
| `block-comment-tokens` | table or table array; unset | `{ start = "...", end = "..." }` pairs for block comments. |
| `text-width` | integer; inherits editor | Language override for `editor.text-width`. |
| `soft-wrap` | table; inherits editor | Language override using the same keys as [`editor.soft-wrap`](#editorsoft-wrap). |
| `auto-format` | boolean; inherits editor | Permit formatting this language on save. Global `editor.format.on-save` must also be enabled. |
| `code-actions-on-save` | string array; unset | LSP code-action kinds run in order on save. |
| `formatter` | table; unset | External formatter. `command` is required; `args` defaults to `[]`. |
| `path-completion` | boolean; inherits editor | Override `editor.completion.path.enable`. |
| `spelling` | table; inherits editor | `languages` and `min-word-length` replace global values; `words` and `ignore-regexes` extend them. See [spell checking](./spell-checking.md). |
| `word-completion` | table; inherits editor | Override `enable` and/or `trigger-length` from `editor.completion.word`. |
| `diagnostic-severity` | `"hint"`, `"info"`, `"warning"`, or `"error"`; `"hint"` | Minimum accepted diagnostic severity. |
| `grammar` | string; defaults to `name` | Tree-sitter grammar name. |
| `injection-regex` | regex string; unset | Match this language at tree-sitter injection sites. |
| `language-servers` | array; `[]` | Server names or `{ name = "...", only-features = [...], except-features = [...] }` entries. |
| `indent` | table; unset | Requires `tab-width` from `1` through `16` and `unit`, the inserted indentation string. |
| `debugger` | table; unset | Debug adapter definition. See below. |
| `auto-pairs` | boolean or character map; inherits editor | Language-specific delimiter pairs. |
| `rulers` | integer array; inherits editor | Language-specific ruler columns. |
| `workspace-lsp-roots` | path array; inherits editor | Language-specific workspace-relative LSP search ceilings. |
| `persistent-diagnostic-sources` | string array; `[]` | Diagnostic sources whose unchanged diagnostics Mitos may track across edits. |
| `rainbow-brackets` | boolean; inherits editor | Language override for rainbow brackets. |

### `[language.debugger]`

See [Debugging (DAP)](./debugging.md) for setup, launch templates, and debugging commands.

| Key | Type / default | Description |
| --- | --- | --- |
| `name` | string; required | Adapter name. |
| `transport` | `"stdio"` or `"tcp"`; required | How Mitos communicates with the adapter. |
| `command` | string; `""` | Adapter executable. |
| `args` | string array; `[]` | Adapter arguments. |
| `port-arg` | string; unset | For TCP, an argument template whose `{}` is replaced with an ephemeral local port. |
| `templates` | array; required | Launch/attach templates exposed by Mitos. |
| `quirks.absolute_paths` | boolean; `false` | Treat paths received from the adapter as absolute. This key retains its underscore spelling. |

Each `[[language.debugger.templates]]` has required `name`, `request`, and
adapter-specific `args` values. Optional `completion` entries are either a
string or `{ name = "...", completion = "...", default = "..." }`; all three
fields in that detailed form are optional.

### `[language-server.<name>]`

| Key | Type / default | Description |
| --- | --- | --- |
| `command` | string; required | Server executable. It must be available on `PATH` unless an absolute path is used. |
| `args` | string array; `[]` | Server arguments. |
| `environment` | string map; `{}` | Environment variables supplied to the server. |
| `config` | table; unset | Server-specific initialization/settings object. |
| `timeout` | seconds; `20` | Maximum request duration. |
| `required-root-patterns` | glob array; unset | Start the server only if at least one pattern exists in the selected LSP root. This validates a root; it does not select one. |

### `[[grammar]]` and `use-grammars`

| Key | Type | Description |
| --- | --- | --- |
| `use-grammars.only` | string array | Fetch/build only the named grammars. Mutually exclusive with `except`. |
| `use-grammars.except` | string array | Fetch/build every grammar except those named. Mutually exclusive with `only`. |
| `grammar.name` | string | Grammar name. |
| `grammar.source.git` | URL | Git repository containing the grammar. |
| `grammar.source.rev` | string | Commit or tag to fetch. Required with `git`. |
| `grammar.source.subpath` | path | Optional subdirectory within a multi-grammar repository. |
| `grammar.source.path` | path | Local grammar directory, as an alternative to the Git source keys. |

`use-grammars` must appear before the array-of-table declarations in TOML.
When omitted, every configured grammar is fetched and built.

## Inline completions

Language servers supporting `textDocument/inlineCompletion` can suggest ghost text
in insert mode. Mitos uses the first configured server with the `inline-completion`
feature. Suggestions leave the document unchanged until accepted and disappear
when you edit, move the cursor, leave insert mode, or switch documents. Requests
pause while the completion popup is previewing an item.

Add bindings to `config.toml` to accept, dismiss, request, or cycle suggestions:

```toml
[keys.insert]
C-y = "inline_completion_accept"
C-e = "inline_completion_dismiss"
C-g = "inline_completion_trigger"
A-n = "inline_completion_next"
A-p = "inline_completion_prev"
```

There are no default bindings. Set `editor.inline-completion-auto-trigger = false`
to request suggestions only with `inline_completion_trigger`. Plain text suggestions
can span multiple lines; snippet suggestions and multiple cursors are not supported.
Ghost text uses the `ui.virtual.inline-completion` theme scope, falling back to
`ui.virtual.inlay-hint`.

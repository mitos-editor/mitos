# Completion and snippets

Mitos suggests words from open buffers and filesystem paths while you type.
Install a [language server](./lsp.md) for code completion and snippets.

## Use the completion menu

Press `Ctrl-x` in insert mode to request completion yourself. When the menu
is open:

| Key | Action |
| --- | --- |
| `Tab`, `Ctrl-n`, `Down` | Select the next suggestion |
| `Shift-Tab`, `Ctrl-p`, `Up` | Select the previous suggestion |
| `Enter` | Accept the selected suggestion |
| `Ctrl-c` | Dismiss the menu and reject the preview |

Typing another character normally accepts the selected suggestion and continues
typing. Mitos previews the selected suggestion in the document by default.
Disable this with `editor.completion.preview-insert = false`.

To request suggestions only with `Ctrl-x`:

```toml
[editor.completion]
auto-trigger = false
```

See the [completion settings](./configuration.md#editorcompletion) for delays,
word completion, and path completion. Remap menu keys under
[`keys.menu`](./remapping.md#component-keys).

## Snippet placeholders

Language servers can return snippets with editable placeholders, such as a
function call's arguments. After accepting one, type over the selected
placeholder and press `Tab` to move to the next one when smart-tab is active
and non-whitespace precedes the cursor.

You can bind placeholder navigation directly, including backward navigation:

```toml
[keys.insert]
A-j = "goto_next_tabstop"
A-k = "goto_prev_tabstop"
```

LSP snippets are enabled by default. To disable them, set
`editor.lsp.completion.snippets = false`, reload configuration, and run
`:lsp-restart`.

## Smart Tab

Outside a snippet, `Tab` moves to the end of the enclosing syntax node when
there is non-whitespace text before the cursor. At the start of a line it
inserts indentation. `Shift-Tab` explicitly inserts indentation with the
default bindings.

While the completion menu is open, `Tab` normally selects the next suggestion.
Set `editor.indent.smart-tab.supersede-menu = true` to give smart-tab priority.
You can still select suggestions with `Ctrl-n` and `Ctrl-p`.

To always insert indentation when the menu is closed:

```toml
[editor.indent.smart-tab]
enable = false
```

## Inline completions

Servers supporting inline completion can show suggestions as faint text after
the cursor. These suggestions leave the document unchanged until accepted.
They disappear when you edit, move, leave insert mode, or switch documents.
Requests pause while the completion menu is previewing an item.

There are no default keys for inline suggestions. Add these to `config.toml`:

```toml
[keys.insert]
C-y = "inline_completion_accept"
C-e = "inline_completion_dismiss"
C-g = "inline_completion_trigger"
A-n = "inline_completion_next"
A-p = "inline_completion_prev"
```

To request suggestions only with your trigger binding:

```toml
[editor.inline-completion]
auto-trigger = false
```

Mitos uses the first configured server with the `inline-completion` feature.
Suggestions can span multiple lines; inline snippet suggestions and multiple
cursors are not supported. See [feature routing](./languages.md#feature-routing)
if you use several servers.

# Completion and snippets

Mitos suggests words from open buffers and filesystem paths while you type.
Install a [language server](./lsp.md) for code completion and LSP snippets, or
define your own [user-defined snippets](./snippets.md).

## Use the completion menu

Press `Ctrl-x` in insert mode to request completion yourself. When the menu
is open:

| Key | Action |
| --- | --- |
| `Ctrl-n`, `Down` | Select the next suggestion |
| `Shift-Tab`, `Ctrl-p`, `Up` | Select the previous suggestion |
| `Enter`, `Tab` | Accept the selected suggestion, or the first match if none is selected |
| `Esc`, `Ctrl-c` | Dismiss the menu and reject the preview, staying in insert mode |

Typing another character normally accepts the selected suggestion and continues
typing. Mitos previews the selected suggestion in the document by default.
Disable this with `editor.completion.preview-insert = false`.

Saving and other commands do not accept preview text. For example, `Ctrl-s`
dismisses the preview and saves the text you entered. After dismissing the menu,
press `Esc` again to return to normal mode.

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
placeholder and press `Tab` to move to the next one, or `Shift-Tab` to move
backward. This works after whitespace too, with smart-tab enabled or disabled.
If a completion menu is open, its keys take priority: accept or dismiss it first.
Use `Ctrl-g n` and `Ctrl-g p` to cycle a placeholder's choices.

You can bind placeholder navigation directly, including backward navigation:

```toml
[keys.insert]
A-j = "snippet_next_tabstop"
A-k = "snippet_previous_tabstop"
```

LSP snippets are enabled by default. To disable them, set
`editor.lsp.completion.snippets = false`, reload configuration, and run
`:lsp-restart`.

## Smart Tab

Outside a snippet or completion menu, `Tab` inserts indentation and `Shift-Tab`
unindents the selected lines. Syntax-aware Tab is optional: when enabled, `Tab`
moves to the end of the enclosing syntax node if non-whitespace text precedes
the cursor, and inserts indentation otherwise.

While the completion menu is open, `Tab` accepts its selected suggestion.
The `supersede-menu` option applies to menu bindings that navigate with Tab;
an explicit accept binding keeps priority. Use `Ctrl-n` and `Ctrl-p` to navigate.

To enable syntax-aware Tab:

```toml
[editor.indent.smart-tab]
enable = true
```

## Inline completions

Servers supporting inline completion can show suggestions as faint text after
the cursor. These suggestions leave the document unchanged until accepted.
They disappear when you edit, move, leave insert mode, or switch documents.
Inline suggestions pause while you preview an item in the completion menu.

There are no default keys for inline suggestions. Add these to `config.toml`:

```toml
[keys.insert]
C-y = "inline_completion_accept"
C-e = "inline_completion_dismiss"
A-n = "inline_completion_next"
A-p = "inline_completion_previous"

[keys.insert.C-g]
t = "inline_completion_trigger"
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

## Snippet variables

Snippet bodies can insert selected text, filenames, dates, and other values.
See the [snippet variable reference](./snippets.md#variables) for supported names
and examples. These variables work in both language-server and user-defined
snippets.

## User-defined snippets

You can [define your own snippets](./snippets.md) without a language server.
Keep them in your Mitos configuration directory or share them with a project.

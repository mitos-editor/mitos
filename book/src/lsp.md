# Language servers

Language servers add completion, error messages, code navigation, renaming,
and formatting. Install them separately from Mitos.

## Get started

1. Find your language in the [language support table](./lang-support.md).
2. Install a listed server.
3. Run `ms --health <language>`, for example `ms --health python`, to check
   that Mitos can find it.
4. Open a file in that language. If Mitos is already running, use `:lsp-restart`.

Some languages list alternatives. Mitos tries them in order and uses the
first one that starts successfully for your project. Separately listed tools
can run alongside it, for example Ruff for Python checking and formatting.

## Common actions

Available features depend on the server.

| Action | Keys or command |
| --- | --- |
| Next / previous error or warning | `]d` / `[d` |
| File / workspace diagnostics | `Space d` / `Space D` |
| Completion | Automatic while typing; `Ctrl-x` opens it manually |
| Documentation at the cursor | `Space k` |
| Parameter hints | Automatic while typing arguments |
| Go to definition / declaration / type | `gd` / `gD` / `gy` |
| Find references / implementations | `gr` / `gi` |
| Rename a symbol | `Space r` |
| Code actions, such as quick fixes | `Space a` |
| File / workspace symbols | `Space s` / `Space S` |
| Format the file | `:format` |

## Interactive refactoring (experimental)

Mitos supports the [experimental interactive refactoring protocol][interactive].
When a language server offers an interactive code action, select it with
`Space-a` and answer the questions that appear. Commands invoked through code
lenses or `:lsp-workspace-command` can ask questions too. Support is negotiated
automatically with the server; gopls v0.23.0 and later provide interactive struct
tag editing and interface implementation.

Questions can ask for text, numbers, yes/no answers, a choice, a searchable choice,
a file or directory, or a list. Defaults are prefilled. File paths are relative to
the source document's directory and must meet the server's existence, type, and
extension constraints. Lists offer Add, Edit, Remove, and Done, including for
nested lists. Leave an optional text field empty or choose Skip to leave it
unanswered. Press `Esc` to cancel the operation.

The server validates answers before executing the command and may ask additional
questions or ask you to correct an answer. Editing or closing the source document,
stopping its server, or starting another interactive command cancels the pending
operation. This protocol is experimental and may change with future server releases.

[interactive]: https://go.googlesource.com/proposal/+/refs/heads/master/design/76331-lsp-interactive-refactoring.md

## Diagnostics and code actions

Use `]d` and `[d` to move between diagnostics, then `Space a` to see fixes
for the current selection. Diagnostics and code actions can also come from
[built-in spell checking](./spell-checking.md), which needs no server.

Warnings on the cursor line can appear below the code:

```text
let total = missing;
            └─ cannot find value `missing`
```

Use [`editor.diagnostics`](./configuration.md#editordiagnostics) to choose
which severities appear inline or at the end of a line. The diagnostic
pickers provide a list even when inline display is disabled.

Code lens labels, such as run or test actions, appear beside code when the
server provides them. Press `Space l` for the commands on the current line.

## Symbols without a server

`Space s` and `Space S` use language-server symbols when available. Otherwise,
Mitos can use syntax queries to list file and workspace symbols. Check the
Code Navigation Tags column in [Language support](./lang-support.md) for available queries.
Syntax symbols do not require a running language server.

## Formatting

Use `:format` for the whole file or `=` for selected ranges supported by the
language server. A configured external formatter takes precedence for whole-file
formatting. Formatting on save requires both `editor.format.on-save` and the
language's `auto-format` setting to be enabled.

See [Languages](./languages.md#configuring-the-formatter-command) for external
formatters and [code actions on save](./languages.md#configuring-code-actions-on-save).
For suggestions while typing, see [Completion and snippets](./completion.md).

## Change settings

Use `:config-open` to edit `config.toml`. For example, to show type hints:

```toml
[editor.lsp.inlay-hints]
enable = true
```

Run `:config-reload` to apply changes. See the
[language-server settings](./configuration.md#editorlsp) for more options,
or set `enable = false` under `[editor.lsp]` to disable language servers.

To choose a server or change its settings, edit
[`languages.toml`](./languages.md#language-server-configuration).
Use `:lsp-restart` afterwards. `:lsp-stop` stops servers for the current file;
`:lsp-workspace-command` runs a command provided by the server.

If a feature is missing, start with the
[troubleshooting guide](./troubleshooting.md#language-features-are-missing).

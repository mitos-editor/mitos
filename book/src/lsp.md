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
| File / workspace diagnostics | `Space-d` / `Space-D` |
| Completion | Automatic while typing; `Ctrl-x` opens it manually |
| Documentation at the cursor | `Space-k` |
| Parameter hints | Automatic while typing arguments |
| Go to definition / declaration / type | `gd` / `gD` / `gy` |
| Find references / implementations | `gr` / `gi` |
| Rename a symbol | `Space-r` |
| Code actions, such as quick fixes | `Space-a` |
| File / workspace symbols | `Space-s` / `Space-S` |
| Format the file | `:format` |

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

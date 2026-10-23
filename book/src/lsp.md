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

## Additional server choices

- For KDL, install [`kdl-lsp`](https://github.com/kdl-org/kdl-rs/tree/main/tools/kdl-lsp)
  and put the executable on `PATH`.
- For OpenTofu files, install [`tofu-ls`](https://github.com/opentofu/tofu-ls).
  Mitos tries `terraform-ls` first and falls back to `tofu-ls`; reorder the
  alternatives in `languages.toml` to prefer OpenTofu.
- For Rego, install [`regal`](https://www.openpolicyagent.org/projects/regal).
  Mitos tries `regols` first, then `regal language-server` with formatting
  enabled. Reorder the alternatives to prefer Regal. Project roots can contain
  `.regal/config.yaml` or `.regal.yaml`.

Run `ms --health kdl`, `ms --health hcl`, or `ms --health rego` to check the
configured executables, then `:lsp-restart` after installing a server.

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

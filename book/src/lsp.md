# Language servers

Mitos has built-in support for the [Language Server Protocol][lsp], providing
IDE-like features — diagnostics, completion, goto-definition, rename and more,
for any language that has a server configured. Language servers are separate
programs you install yourself: the [language support](./lang-support.md) page
lists which languages ship with a server configured, and the
[language server configuration wiki][wiki] has per-server installation notes.

Run `ms --health <language>` to check whether Mitos found the configured server
for a language. After changing a server's configuration, `:lsp-restart` reloads
it and `:lsp-stop` stops it; `:lsp-workspace-command` runs a command the server
exposes for the workspace.

## Features

Most features are bound by default (see the [keymap](./keymap.md)); the relevant
keys are listed below.

| Feature | Default key / command |
| --- | --- |
| Diagnostics — shown inline and in the gutter | navigate with `[d` / `]d`; `Space-d` / `Space-D` open the document / workspace pickers |
| Completion | automatic while typing; `Ctrl-x` triggers it manually |
| Hover documentation | `Space-k` |
| Signature help (parameter hints) | automatic while typing arguments |
| Goto definition / declaration / type / references / implementation | `gd` / `gD` / `gy` / `gr` / `gi` |
| Rename symbol | `Space-r` |
| Code actions | `Space-a` |
| Document / workspace symbols | `Space-s` / `Space-S` |
| Format document | `:format`, or set `format.on-save` to format on save |
| Inlay hints | enable with `lsp.inlay-hints.enable` (see below) |

## Configuration

The [`[editor.lsp]`](./editor.md#editorlsp-section) section of `config.toml`
groups LSP behaviour into feature subsections, including `inlay-hints.enable`,
`signature-help.auto-trigger`, `document-highlight.auto-trigger`, and
`completion.snippets`. Set `enable = false` to disable LSP entirely. How
much diagnostic detail is shown inline at the cursor is configured separately in
[`[editor.diagnostics.inline]`](./editor.md#editordiagnosticsinline-section).

Which server(s) a language uses, the server's command and arguments, and any
language-specific settings are configured in `languages.toml` (see
[Language Server configuration](./languages.md#language-server-configuration)). A
language may use [several servers](./languages.md#configuring-language-servers-for-a-language):
for each request the first server in the list that supports it is used, and a
server's features can be limited with `only-features` / `except-features`.

[lsp]: https://microsoft.github.io/language-server-protocol/
[wiki]: https://github.com/helix-editor/helix/wiki/Language-Server-Configurations

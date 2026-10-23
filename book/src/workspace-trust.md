# Workspace trust

A project can configure programs that Mitos runs. Workspace trust lets you
review project settings before loading them. The default policy allows language
servers from your user configuration and debug adapters you launch, but requires
trust before loading `.mitos/config.toml`, `.mitos/languages.toml`, `.mitos/snippets.toml`, or
repository-local Git settings that can run external commands.

Trust is not a sandbox for language servers or debug adapters. Those tools may
read project files and run programs of their own.

## Granting trust

When opening a file whose project settings need trust, Mitos offers:

- **Trust** to remember approval for the workspace.
- **Never** to exclude it and stop future prompts.
- `Esc` to leave it untrusted for this session. Mitos may ask again next session.

A `[⚠]` indicator appears in the bottom-right when the workspace is restricted
and granting trust would enable project settings or a blocked language server.
Run `:workspace-trust` to approve a workspace after reviewing its settings.

## Revoking trust

Use `:workspace-untrust` to remove a grant or exclusion. Use
`:workspace-exclude` to stop prompts for a workspace without trusting it.

## Detecting changes after trust was granted

When files under `.mitos/` change, Mitos leaves the changed project settings
unloaded until you approve them again. Review the changes, then run
`:workspace-trust` to approve them.

Servers allowed by your user policy may still run. An approval of changed
configuration is separate from permission to use your globally configured tools.

## Configuration

Trust policy is read only from your user `config.toml`; project files cannot
weaken it. The default is:

```toml
[editor.workspace-trust]
level = "servers"
prompt = true
```

| Level | What may run without a workspace grant |
| --- | --- |
| `"none"` | No language servers, debug adapters, or gated project settings |
| `"servers"` | Language servers and manually launched debug adapters |
| `"insecure"` | All project settings and tools, without approval or change detection |

To approve each workspace by command instead of answering prompts:

```toml
[editor.workspace-trust]
level = "none"
prompt = false
```

The restriction indicator remains visible even when prompts are disabled.

### Trusting workspaces by path

Use path globs only for directories whose contents you trust:

```toml
[editor.workspace-trust]
trusted = ["~/src/github.com/me/*"]
```

`~` and environment variables are expanded. Matching workspaces are trusted
without checking later `.mitos/` changes, including repositories added to the
matching directory in the future. Explicit approval per workspace preserves
change detection. An explicit `:workspace-exclude` overrides a path pattern.

## Git trust

Git change tracking still works in untrusted workspaces, including built-in
line-ending conversion. Repository-local settings that could launch external
filter programs are ignored until you trust the workspace.

## Storage

Remembered decisions live in `~/.local/share/mitos/workspace_trust/` on Linux
and macOS, or `%AppData%\mitos\workspace_trust\` on Windows. On Unix,
`XDG_DATA_HOME` can change the data directory. Use the trust commands to manage
decisions rather than editing these files.

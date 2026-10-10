# Plugin permissions

A plugin asks for capabilities in its `plugin.toml`. You grant them separately
in your user `config.toml`. An operation is allowed only when both lists include
its capability. Enabling a plugin grants only `ui` by default; it does not grant
access to document text, files, programs, environment variables, private storage,
or the clipboard.

Follow the package's instructions and grant only the features you want to use.
A capability listed as required by the package must be granted before it can
load. Other features may remain available when you leave optional capabilities
out; the plugin receives a permission error if it tries to use one you withheld.

## Grant capabilities

For a hypothetical text-editing package installed as `example`, a user grant
might be:

```toml
[plugins.example]
path = "plugins/example/plugin.toml"

[plugins.example.permissions]
capabilities = ["editor-read", "editor-edit", "editor-selection", "ui"]
```

An explicit `capabilities` array replaces the default list. Include `ui` yourself
if the package needs it. Reload with `:config-reload`, then use
`:plugin-inspect example` to see the package's declared and effective permissions.

| Capability | What it allows |
| --- | --- |
| `editor-read` | Read document text, selections, document lists, and editor events that contain document information |
| `editor-edit` | Edit document text, including documents open in the editor but not visible in a split |
| `editor-selection` | Change selections and cursors, and write ordinary editor registers |
| `editor-navigate` | Open, focus, split, and close documents or views through supported editor operations |
| `editor-settings` | Temporarily override supported settings such as soft wrap, cursorline, auto-format, and theme |
| `ui` | Show status messages, prompts, and pickers, and contribute keybindings |
| `workspace-read` | Read files and search within your `read-roots` |
| `workspace-write` | Write files within your `write-roots` |
| `storage` | Read and write private persistent data belonging to the configured plugin ID |
| `process` | Run programs matching your `processes` rules, using an explicitly configured working directory |
| `environment` | Pass only the environment variables named in `environment` to an allowed program |
| `clipboard` | Read and write the native clipboard; a custom command-based clipboard provider also needs a matching process grant |
| `network`, `provider` | Reserved for future services; granting them does not currently enable network requests or provider registration |

Some operations need more than one capability. Opening a file through a plugin,
for example, needs both `editor-navigate` and `workspace-read`. Reading an ordinary
register needs `editor-read`; clipboard registers additionally need `clipboard`.
Opening a file at a chosen cursor position also needs `editor-selection`.
Permission errors can be reported even without `ui`.

`editor-edit` is not limited by filesystem roots: it applies to documents already
open in Mitos. Changes to documents opened normally follow your existing save
and autosave settings. A later native save can therefore write a plugin's edits
to disk even when the plugin has no `workspace-write` grant.

## Files and directory roots

File access has no implicit project, current-directory, or home-directory grant.
Add the directories the package should use. The permission tables below belong
to existing plugin entries; keep each entry's `path` and other settings. For
example, a package that searches one project could use:

```toml
[plugins.search.permissions]
capabilities = ["editor-read", "editor-selection", "editor-navigate", "workspace-read", "ui"]
read-roots = ["/home/you/projects/notes"]
```

This is an illustration, not a bundled package. Use the configured ID and paths
for your own package. The directories must exist when plugins load. The package's
instructions should explain how it selects a root when you grant several.

A write grant is separate:

```toml
[plugins.exporter.permissions]
capabilities = ["workspace-write", "ui"]
write-roots = ["/home/you/exports"]
```

Use actual absolute paths, or paths relative to the standard Mitos configuration
directory. On Linux and macOS that directory is `$XDG_CONFIG_HOME/mitos`, falling
back to `~/.config/mitos`; on Windows it is `%AppData%\mitos`. A custom `--config`
file does not change this base. Environment-variable names and `~` inside these
TOML path strings are not expanded.

Host file operations stay within their granted directories, including when
following symbolic links. Devices and named pipes are rejected. A plugin-opened
file is read through that scoped access. It does not automatically start language
servers, discover project tools, or gain normal automatic reload and autosave
behavior. A deliberate native open, reload, save, or provider restart can adopt
normal editor authority. Plugin file opening currently rejects binary files.

These directory restrictions apply to host file services. They do not sandbox a
program you separately allow the plugin to run.

## Running native programs

A formatter might need this grant, adjusted to the exact program and arguments
it uses:

```toml
[plugins.formatter.permissions]
capabilities = ["editor-read", "editor-edit", "process", "ui"]
read-roots = ["/home/you/projects/notes"]
processes = [{ command = "/usr/bin/my-formatter", args = ["--stdin"] }]
```

The example program name is a placeholder. Both its name or path and its argument
list must match the plugin's request exactly. A final `"**"` in an argument rule
allows any trailing arguments; use it only when that wider grant is intentional.
For example, `["--stdin", "**"]` allows extra arguments after `--stdin`.

Programs use a working directory selected from `read-roots`; a process needs one
even if it does not use the `workspace-read` service. The host passes arguments
directly, without shell parsing. Allowing a shell program explicitly still gives
that shell the authority to execute its arguments.

Programs receive piped input and output and an empty environment by default. To
allow particular variables, grant `environment` as well as `process`, then list
only the names needed:

```toml
[plugins.formatter.permissions]
capabilities = ["editor-read", "editor-edit", "process", "environment", "ui"]
read-roots = ["/home/you/projects/notes"]
processes = [{ command = "/usr/bin/my-formatter", args = ["--stdin"] }]
environment = ["LANG"]
```

An allowed native program runs with the operating-system permissions of Mitos.
It can perform its own file or network operations outside your plugin directory
roots. WebAssembly isolation does not make native tools an OS sandbox. Process
jobs are supported only on platforms where Mitos can cancel and clean up their
process tree; otherwise the service reports `unsupported-interface`.

## Private storage and clipboard

`storage` gives a package private persistent key-value data under the configured
plugin ID. On Linux and macOS its directory is
`$XDG_DATA_HOME/mitos/plugins/<id>`, or `~/.local/share/mitos/plugins/<id>` when
unset. On Windows it is `%AppData%\mitos\plugins\<id>`. The plugin cannot choose
another storage directory. Renaming its configured ID selects a different
namespace. Reloading, disabling, or removing an entry does not delete stored data.
Values are limited to 64 KiB, with at most 128 keys and 1 MiB per plugin.

Clipboard access is separate from document access. Native clipboard requests
are limited to 4 KiB of text and run away from the editor thread. If your clipboard
configuration uses a custom executable, the plugin also needs `process` and a
rule matching that executable and its arguments. Some clipboard operations depend
on the terminal or operating system and cannot be interrupted immediately.

## Updates and digest pins

By default, your permissions continue to trust the same configured package path
when its files change. An update can use the permissions already granted to it;
newly requested capabilities still need your grant.

An optional `sha256` entry pins the exact component file:

```toml
[plugins.example]
path = "plugins/example/plugin.toml"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
```

Replace that illustrative digest with the component's actual 64-digit SHA-256.
Use `sha256sum component.wasm` on Linux, `shasum -a 256 component.wasm` on macOS,
or `Get-FileHash component.wasm -Algorithm SHA256` in PowerShell. Compare it with
a digest from a source you trust before granting access.

A mismatch prevents loading the replacement. The pin covers the component bytes,
not its manifest or asset files. It is unavailable for packages with no component.
When updating a pinned package, verify the new file and update the pin before
reloading.

## Workspace configuration

Only your user configuration grants permissions. This includes the file selected
with `--config`. A trusted workspace's `.mitos/config.toml` may override a plugin
entry by ID, subject to these rules:

- The same ID and the same configured path keep your user permissions and digest
  pin; workspace values for those fields are ignored.
- A different path receives only the default `ui` grant and no digest pin.
- An entry present only in the workspace receives only the default `ui` grant.
- A same-path workspace override cannot enable an entry you disabled in your user
  configuration.

[Workspace trust](./workspace-trust.md) decides whether workspace configuration
loads at all. It does not grant plugin capabilities. Plugins are loaded from
explicit entries, never by scanning a project for modules.

## Limits and failures

Permissions do not remove resource limits. Text reads, file operations, jobs,
output, storage, and UI have separate bounds. A denied capability, changed
selection, stale document version, or exceeded limit produces an error instead
of silently granting more access. A guest trap disables that plugin until reload;
other plugins remain available.

For a failure, start with `:plugin-inspect <id>` and `:plugin-logs <id>`. See
[Using plugins](./plugins.md#troubleshooting) for the normal troubleshooting flow.
The [Plugin API contract](./plugin-contract.md) describes the detailed service
and lifecycle rules for authors.

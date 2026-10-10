# Plugin permissions

A package declares the authority it needs in `plugin.toml`. Your global Mitos
configuration grants that authority independently. The host requires both on
every operation, including operations performed by a helper or builtin command.
Enabling a plugin grants only `ui` by default. File, process, network,
environment, clipboard, and persistent-storage access are denied by default.

For an editor transform, the manifest contains:

```toml
capabilities = ["editor-read", "editor-edit", "editor-selection", "ui"]
```

The corresponding user configuration contains:

```toml
[plugins.uppercase]
path = "plugins/uppercase/plugin.toml"

[plugins.uppercase.permissions]
capabilities = ["editor-read", "editor-edit", "editor-selection", "ui"]
```

| Capability | Authority |
| --- | --- |
| `editor-read` | Document text, selections, catalog, and document observations |
| `editor-edit` | Revision-checked document transactions |
| `editor-selection` | Revision-checked selection and cursor changes |
| `editor-navigate` | Explicit document/view navigation |
| `editor-settings` | Owned overrides of the supported editor settings |
| `ui` | Bounded status, prompt, picker, and keymap contributions |
| `workspace-read` | Files and search under explicitly granted read roots |
| `workspace-write` | Writes under explicitly granted write roots |
| `storage` | Private plugin storage with its own quota |
| `process` | Native executable and argument rules, selected working directory |
| `environment` | Only explicitly listed environment variables |
| `clipboard` | Bounded native clipboard read/write; custom command providers also require the exact process grant |
| `network`, `provider` | Reserved authority; a grant does not create an unavailable service |

Workspace configuration cannot grant authority. An override of the same plugin
path keeps the user's permissions and optional module digest pin; an override to
a different path loses those grants. A workspace cannot enable a plugin the
user explicitly disabled. Workspace-only entries receive default UI permission.
Workspace trust remains an independent requirement for loading workspace config.
Relative package and permission-root paths resolve from the user config directory.

The optional `sha256` in a global plugin entry pins the module bytes. A mismatch
rejects the replacement. Without a pin, your grant trusts the configured package
path across package updates. A same-path update can use only authority you already
granted. Replacing the running generation revokes its old handles and jobs.

## Files and storage

There is no implicit current-directory or home-directory grant. Configure
`read-roots` and `write-roots` explicitly. File operations use opened directory
handles and reject absolute or parent-relative paths inside those handles.
Symlink resolution cannot escape the granted directory. Editor opening consumes
the bytes read through that handle, rather than checking a path and reopening it.
Only regular files are read or written; FIFOs and devices are rejected.

Documents first opened through a plugin retain restricted adoption: automatic
reload, autosave, `.editorconfig` discovery, VCS discovery, and provider startup
cannot reopen their bookkeeping paths. An explicit native open, reload, save, or
provider restart adopts normal user authority. Plugin opening currently rejects
binary files. Syntax grammars, queries, and spelling dictionaries remain existing
host-configured native services; plugin packages cannot supply a native DLL path.
Closed builtin composition applies the same read-only and binary-buffer guards
as direct plugin transactions. Registers require editor-read/editor-selection
as appropriate; clipboard reads and writes always require clipboard authority.
The native clipboard adapter admits at most two requests with 4 KiB input/output
and a two-second admission/execution deadline. Native Windows clipboard IPC and
terminal OSC52 writes retain their worker permit until the underlying operation
finishes; they cannot promise hard interruption of a blocked OS call.

Edits to documents already opened natively follow the user's existing save and
autosave policy.

Private storage uses hashed keys within a host-owned plugin directory. Keys have
at most 256 bytes, values 64 KiB, and a plugin has at most 128 keys and 1 MiB of
stored data. Writes replace a complete value. Durable storage survives instance
reload; guest linear memory does not. Guests cannot choose a storage directory.

## Native tools

An example explicit tool grant is:

```toml
[plugins.formatter.permissions]
capabilities = ["editor-read", "editor-edit", "process", "ui"]
read-roots = ["/home/me/project"]
processes = [{ command = "/usr/bin/my-formatter", args = ["--stdin"] }]
```

Commands are exact executable names or paths; arguments must match exactly. A
final `"**"` explicitly grants arbitrary trailing arguments. No shell expression
is parsed by the host. Tools receive piped input/output, an explicit working
directory, and an empty environment unless `environment` and specific names are
also granted. No raw terminal input/output is exposed.

Native execution gives the tool the operating-system authority of the editor
process. An executable allowlist does not restrict the tool's own file or network
access. Mitos does not claim an OS sandbox for these tools. Unix process jobs
run in their own process group, terminate the group on timeout/cancellation, and
reap the child. Platforms without implemented process-tree cleanup reject this
service with `unsupported-interface`.

## Declarative assets

Packages can contribute bounded themes, static snippets, syntax query sources,
and closed language profiles without an executable module. Profiles use already
installed, host-approved grammars. They cannot configure formatters, language
servers, debuggers, or native DLL paths. Native user registrations take precedence;
owned overlays are removed or restored on unload. Query preflight limits source
complexity before native compilation and rejects regex predicates. It does not
turn native query compilation/execution into interruptible WASM work.

## Resource and failure policy

Host jobs are limited independently of guest instruction budgets. A plugin has
at most 16 active jobs and an editor at most 32 across packages and replacement
generations. Filesystem work is serialized through a shared host I/O budget.
Search visits at most 4,096 entries and 64 MiB, emits
chunks of at most 64 matches, and retains at most eight result chunks. Tool
arguments have at most 64 entries and 64 KiB; tool input has at most 1 MiB;
stdout and stderr each have at most 1 MiB. Search and tools have a ten-second
deadline; timers are limited to 60 seconds. Directory grants have at most 16
roots of each kind. Quotas reject work rather than silently truncating mutations.
Search explicitly reports truncated results.

No HTTP, archive extraction, ambient WASI, package-install script, arbitrary
shell, or generic editor-command RPC is exposed. New services must define and
enforce their own authority, cancellation, and byte limits before being enabled.

Failures distinguish stale state, denied permission, cancellation, deadline,
exhausted resources, unsupported services, guest traps, and host failures. A
stale edit or denied request rejects the response before document mutation.
Host-created oversized input does not disable a healthy guest. Guest traps
discard the affected store and its pending effects; other plugins remain usable.

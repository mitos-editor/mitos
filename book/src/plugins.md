# Plugins

Plugins add commands, editing tools, prompts, and pickers to Mitos. Packages can
also provide themes, snippets, and language highlighting without running plugin
code. Executable plugins use WebAssembly components.

Plugins are optional: Mitos loads only the packages you list in your
configuration. The plugin API is experimental, so use packages built for your
version of Mitos.

## Build or install Mitos

Use a Mitos build that includes plugin support. If you are building this checkout,
run this from the repository root:

```sh
cargo build -p term --bin ms --release --locked
```

Start `target/release/ms` (`target/release/ms.exe` on Windows). Follow
[Building from source](./building-from-source.md) to set up the runtime files,
then run `ms --health` to check your installation. No extra Cargo feature is
needed for plugins. Do not add `--features integration`: that feature is for the
headless test frontend, not the editor you use in a terminal.

Installing a ready-made plugin does not require Rust or a WebAssembly compiler.

## Install a package

Obtain a compatible package from its author and copy the complete package into a
directory you control. Keep its `plugin.toml`, component, and any theme, snippet,
or other asset files together. A plain core-WASM binary is not a Mitos component;
use the packaged component supplied by the author.

A convenient location is a `plugins` subdirectory of your Mitos configuration
directory:

| Platform | Configuration directory |
| --- | --- |
| Linux and macOS | `$XDG_CONFIG_HOME/mitos`, or `~/.config/mitos` when unset |
| Windows | `%AppData%\mitos` |

For example, a package might be arranged like this:

```text
mitos/
  config.toml
  plugins/
    example/
      plugin.toml
      example.component.wasm
      ...other package files...
```

The following examples assume this hypothetical package provides a command named
`tidy`. Replace the names and permissions with those in your package's
instructions.

Open your user configuration with `:config-open` and add:

```toml
[plugins.example]
path = "plugins/example/plugin.toml"
enabled = true
config = {}

[plugins.example.permissions]
capabilities = ["editor-read", "editor-edit", "editor-selection", "ui"]
```

`path` may point to the manifest or to the directory containing `plugin.toml`.
Absolute paths work too. Relative paths always start at the standard
configuration directory above, even when you launch Mitos with
`--config /another/location/config.toml`. `--config` changes the user configuration
file; it does not change the base directory for plugin paths or permission roots.

`enabled` defaults to `true`. The optional `config` table contains the plugin's
own settings; follow its documentation for supported keys. The permissions in
this example let a text-editing plugin read buffers, edit text, update selections,
and show UI. They are not a requirement for every plugin. Without an explicit
permissions list, only `ui` is granted. Read
[Plugin permissions](./plugin-permissions.md) before granting access to files,
programs, environment variables, or the clipboard.

Save the configuration and run `:config-reload`. Loading happens in the
background; commands become available when the package finishes loading. Use
`:plugin-inspect example` to check its status if a command does not appear.

## Run commands

The name in `[plugins.example]` is the plugin's ID in your configuration. It
prefixes its commands, so the package's `tidy` command becomes `:example.tidy`.
Changing that ID changes its command names, contributed theme and language names,
and private storage location.

There are several ways to run a plugin command:

- Press `:` in normal mode, type `example.tidy`, and press Enter.
- Start typing `:example.` and use Tab to complete a command name. Descriptions
  and any argument choices supplied by the package appear in the command prompt.
- Press Space followed by `?` to open the default command palette, then search for
  `example.tidy`. Commands that require arguments are easier to run from `:`.
- Bind the command to a key in `config.toml`.

For example, bind Alt+t in normal and select modes:

```toml
[keys.normal]
"A-t" = ":example.tidy"

[keys.select]
"A-t" = ":example.tidy"
```

Keep the leading `:` in a keybinding. Add entries to existing key tables rather
than declaring the same table twice. See [Key remapping](./remapping.md) for other
keys and sequences. Your keybindings take precedence over bindings supplied by a
plugin.

Plugin arguments use the normal [command-line quoting and
expansions](./command-line.md). For example, `:example.choose 'two words'` passes
one argument if the package provides a `choose` command. Argument completion
uses choices declared by the package; it does not run plugin code.

You can also create a shorter [custom command](./custom-commands.md):

```toml
[commands]
":tidy" = ":example.tidy"
```

After `:config-reload`, `:tidy` runs the plugin command. If a custom command
shadows a plugin command's full name, prefix it with `^` to call the plugin
directly, for example `:^example.tidy`.

Plugin work can finish after the command prompt closes. Wait for a formatter's
completion before saving if you want its changes included in that write. An edit
can be rejected when you have changed the document or selection while the plugin
was working; this protects newer input.

## Themes, snippets, and language packages

Install and configure these packages the same way. A package with only themes,
snippets, or highlighting files needs no WebAssembly component.

A theme named `tone` from `[plugins.example]` is available as `example.tone`:

```text
:theme example.tone
```

Snippets appear through the editor's completion UI for their configured language
and use native snippet tabstops. Language packages use grammars already installed
in Mitos. They cannot install a native grammar library or configure a language
server or formatter. Your native file associations take precedence over package
associations.

Disabling or removing a package removes its contributed commands, bindings, and
assets. Temporary settings owned by the plugin are restored to the current user
settings.

## Update, reload, or disable

Use `:config-reload` after changing paths, settings, permissions, keybindings, or
which plugins are enabled. To reload changed package files without rereading
`config.toml`, run `:plugin-reload`. This command reloads all configured plugins;
it does not accept a plugin name.

Replace a package's files together, then reload. Reload clears its in-memory
state and cancels its unfinished dialogs and jobs. Private storage survives
reload. If replacement preparation fails, the previous working plugins remain
active; inspect the error before retrying. A plugin that traps or is interrupted
is disabled until a successful reload.

To disable a package, keep its entry and change:

```toml
[plugins.example]
path = "plugins/example/plugin.toml"
enabled = false
```

Then run `:config-reload`. Removing the entry and reloading also unloads it;
neither action deletes its package files or private stored data.

Trusted workspace configuration may override plugin settings, but it cannot
grant more permissions. Mitos does not scan project directories for plugins or
run a package just because it is present. See [Workspace trust](./workspace-trust.md)
and [Plugin permissions](./plugin-permissions.md) for the exact rules.

## Troubleshooting

These commands open reports in a scratch buffer. Supply your configured plugin
ID to focus on one package, or omit it to see all packages:

| Command | Shows |
| --- | --- |
| `:plugin-inspect [name]` | Load status, declared and effective permissions, and pending work |
| `:plugin-logs [name]` | Recent plugin messages and failures |
| `:plugin-timings [name]` | Time spent waiting, executing the plugin, and applying its result |

For a missing command, check the configured ID, manifest path, load status, and
whether the package actually declares that command. For a permission error,
compare the package's requested permissions with your grants and check that any
configured directory exists. For an incompatible API or binary, obtain a package
built for this Mitos version. `:log-open` opens the editor log when you need more
detail.

Mitos limits plugin execution, memory, text reads, jobs, and output. A resource
limit or deadline error means that work was rejected or stopped; it is not a
request to grant broader permissions. Report repeated failures to the package's
author with the plugin ID, Mitos version (`ms --version`), and relevant log entry.

## For plugin authors

The [Plugin API contract](./plugin-contract.md) describes commands, events,
revision checks, services, and lifecycle rules. It is an advanced reference for
authors; installing a package does not require learning the author API.

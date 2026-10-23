# Starting Mitos

The executable is `ms`. Run `ms --help` for the available flags.

## Open files and directories

```sh
ms README.md
ms README.md Cargo.toml
ms .
ms src/main.rs:42:5
```

Multiple files open as buffers; use `Space b` to switch between them. Opening a
directory sets the working directory and opens the file picker. A position
can follow a file path as `:line` or `:line:column`; both start at 1.

Use `ms +42 src/main.rs` to open the first file at line 42, or
`ms + src/main.rs` to start at its last line. Use `--` before a filename that
starts with a dash: `ms -- -notes.txt`.

## Choose the layout and working directory

```sh
ms --vsplit README.md Cargo.toml
ms --hsplit README.md Cargo.toml
ms --working-dir /path/to/project src/main.rs
```

`--vsplit` and `--hsplit` open each file in a separate view.
`-w` is the short form of `--working-dir`. File arguments are resolved relative
to the shell's directory before the working directory is changed.
See [Files, buffers, and splits](./files.md) for managing them inside Mitos.

## Read piped input

Pipe text into Mitos to edit it in a scratch buffer:

```sh
git diff | ms
```

Use `:write <path>` to save the text. This requires an interactive terminal
for the editor interface.

## Setup and troubleshooting flags

| Flag | Purpose |
| --- | --- |
| `-t <file>` / `--theme <file>` | Use a theme file for this session, including after configuration reloads |
| `--tutor` | Open the interactive tutorial |
| `-c <file>` / `--config <file>` | Use another `config.toml` |
| `--health [category]` | Check setup; use a language name, `clipboard`, `languages`, `all-languages`, or `all` |
| `-g fetch` / `--grammar fetch` | Download configured grammar sources |
| `-g build` / `--grammar build` | Compile configured grammars |
| `--strict` | Exit on an error in operations such as grammar fetching |
| `--migrate-config <file> --stdout` | Export migrated configuration without rewriting the input |
| `-v`, `-vv`, `-vvv` | Increase log detail |
| `--log <file>` | Choose a log file |
| `-V` / `--version` | Print the version |
| `-h` / `--help` | Print usage information |

See [Troubleshooting](./troubleshooting.md) for health checks and collecting
logs, and [Configuration migration](./configuration-migration.md) for upgrades.

Theme file paths resolve relative to the shell's starting directory. Inherited
parent themes use the usual user/runtime theme search paths. An unreadable or
invalid override falls back to the default theme and records the error in the log.

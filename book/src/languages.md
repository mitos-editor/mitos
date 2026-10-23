# Languages

Use `languages.toml` to change language-specific settings, choose language
servers, or configure a formatter. Most languages already have defaults; you
only need to add the settings you want to change.

For installation help, see [Language servers](./lsp.md). The
[configuration reference](./configuration.md#languagestoml-reference) lists
all available settings.

## `languages.toml` files

Create `languages.toml` in your [configuration directory](./configuration.md)
to change your defaults:

- Linux and macOS: `~/.config/mitos/languages.toml`
- Windows: `%AppData%\mitos\languages.toml`

For settings that apply to one project, use `.mitos/languages.toml` in that
project. Project settings override your own and are loaded after you
[trust the workspace](./workspace-trust.md).

## Language configuration

Add a `[[language]]` section with the language's name and the settings you
want to change. For example, to disable formatting on save for Rust:

```toml
[[language]]
name = "rust"
auto-format = false
```

Use the names in the [language support table](./lang-support.md).
For example, JavaScript, JSX, TypeScript, and TSX have separate entries.

## Project and LSP root selection

Mitos usually finds the project folder automatically. A language's `roots`
setting lists project files such as `Cargo.toml` or `package.json`. For a file
inside nested projects, Mitos uses the outermost matching folder within the
workspace.

For a monorepo where servers should treat subfolders as separate projects,
add this to `.mitos/config.toml`:

```toml
[editor.lsp]
workspace-roots = ["frontend", "backend"]
```

You can also set `workspace-lsp-roots` in a language entry in
`.mitos/languages.toml` to apply this to one language.

Some servers have `required-root-patterns`, such as a database configuration
file. Mitos starts those servers only when a pattern matches in the selected
project folder.

### File-type detection and the `file-types` key

Mitos determines which language configuration to use based on the `file-types` key
from the above section. `file-types` is a list of strings or tables, for
example:

```toml
file-types = ["toml", { glob = "Makefile" }, { glob = ".git/config" }, { glob = ".github/workflows/*.yaml" } ]
```

When determining a language configuration to use, Mitos searches the file-types
with the following priorities:

1. Glob: values in `glob` tables are checked against the full path of the given
   file. Globs are standard Unix-style path globs (e.g. the kind you use in Shell)
   and can be used to match paths for a specific prefix, suffix, directory, etc.
   In the above example, the `{ glob = "Makefile" }` config would match files
   with the name `Makefile`, the `{ glob = ".git/config" }` config would match
   `config` files in `.git` directories, and the `{ glob = ".github/workflows/*.yaml" }`
   config would match any `yaml` files in `.github/workflow` directories. Note
   that globs should always use the Unix path separator `/` even on Windows systems;
   the matcher will automatically take the machine-specific separators into account.
   If the glob isn't an absolute path or doesn't already start with a glob prefix,
   `*/` will automatically be added to ensure it matches for any subdirectory.
2. Extension: if there are no glob matches, any `file-types` string that matches
   the file extension of a given file wins. In the example above, the `"toml"`
   config matches files like `Cargo.toml` or `languages.toml`.

### Configuring the formatter command

External formatters have a 20-second timeout. Set `timeout` (in seconds) in the
`formatter` table to allow a longer or shorter runtime. A timed-out formatter is
terminated; saving still writes the buffer without its formatting changes.

[Command line expansions](./command-line.md#expansions) are supported in the arguments
of the formatter command. In particular, the `%{buffer_name}` variable can be passed as
argument to the formatter:

```toml
formatter = { command = "mylang-formatter" , args = ["--stdin", "--stdin-filename", "%{buffer_name}"] }
```

Expansions use the document being formatted, including its filename and directory.
Selection and cursor expansions use the view that requested formatting. They
require a view, so they are unavailable when saving a hidden document. Shell
expansions run in the formatter's working directory.

### Configuring code actions on save

The `code-actions-on-save` key lists LSP code actions to run (in order) when
the buffer is saved. The available actions depend on available language servers,
below are common examples and may need adjusting for your setup.

```toml
# Python with ruff
[[language]]
name = "python"
code-actions-on-save = ["source.organizeImports", "source.fixAll"]

# TS/JS with tsserver
[[language]]
name = "typescript"
code-actions-on-save = ["source.addMissingImports.ts"]

# TS/JS with eslint
[[language]]
name = "javascript"
code-actions-on-save = ["source.fixAll.eslint", "source.organizeImports"]
```

## Language Server configuration

### Configuring Language Servers for a language

Set `language-servers` to choose which tools a language uses:

```toml
[[language]]
name = "python"
language-servers = ["basedpyright", "ruff"]
```

This runs both servers when installed. Use
[alternatives](#ordered-alternatives-and-supplementary-servers) to choose
between servers that provide the same features.

### Change a server's command or settings

Most servers already have a definition. Add only the settings you want to
change under `[language-server.<name>]`:

```toml
[language-server.basedpyright]
command = "/path/to/basedpyright-langserver"
args = ["--stdio"]
```

For a new server, define its `command` here, then add its name to the
language's `language-servers` list. The command must be on your `PATH` or be
an absolute path.

Use `config` for the server's own settings. Their names and values depend
on the server; check its documentation. For example:

```toml
[language-server.rust-analyzer.config]
check.command = "clippy"
```

Run `:lsp-restart` after changing server settings. See the
[server configuration reference](./configuration.md#language-servername)
for arguments, environment variables, timeouts, and project requirements.

### Ordered alternatives and supplementary servers

Use `alternatives` to choose one server from a list, in preference order.
List other servers separately to run them alongside it:

```toml
[[language]]
name = "python"
language-servers = [
  { alternatives = ["ty", "basedpyright", "pylsp"] },
  { name = "ruff", only-features = ["diagnostics", "code-action", "format"] },
]
```

Here, Mitos uses the first installed server that can start for the project,
plus Ruff for checking and formatting. If a server fails to start, Mitos tries
the next alternative. Each group needs at least one server; use each server
name only once when configuring alternatives.

Run `ms --health python` to check which servers are installed and suitable for
the current project. This checks availability without starting them.

Use `:lsp-restart` to retry the list after installing or fixing a server, or
`:lsp-restart basedpyright` to choose a specific alternative. To make that
choice permanent, change `language-servers` in your `languages.toml`.

Once a server starts, Mitos keeps using it until you run `:lsp-restart`.
If it stops working, use that command to recover. `:lsp-stop` keeps it
stopped until you restart it.

### Feature routing

When servers run together, use `only-features` or `except-features` to choose
what each one provides. For example, to use Ruff only for formatting:

```toml
[[language]]
name = "python"
language-servers = [
  "basedpyright",
  { name = "ruff", only-features = ["format"] },
]
```

Without a filter, all features the server supports are enabled. You can also
put filtered entries inside an `alternatives` list.

For navigation, hover, and formatting, Mitos uses the first listed server
that supports the feature. Completion, diagnostics, code actions, code lenses,
and symbol lists combine results from all enabled servers.

Available feature names:

| Purpose | Names |
| --- | --- |
| Formatting | `format` |
| Navigation | `goto-declaration`, `goto-definition`, `goto-type-definition`, `goto-reference`, `goto-implementation`, `call-hierarchy` |
| Hints and documentation | `hover`, `signature-help`, `document-highlight`, `inlay-hints`, `inline-values`, `document-colors` |
| Completion | `completion`, `inline-completion` |
| Actions | `code-action`, `code-lens`, `rename-symbol`, `workspace-command`, `document-links` |
| Symbols | `document-symbols`, `workspace-symbols` |
| Diagnostics | `diagnostics`, `pull-diagnostics` |

## Tree-sitter grammar configuration

The source for a language's tree-sitter grammar is specified in a `[[grammar]]`
section in `languages.toml`. For example:

```toml
[[grammar]]
name = "mylang"
source = { git = "https://github.com/example/mylang", rev = "a250c4582510ff34767ec3b7dcdd3c24e8c8aa68" }
```

Grammar configuration takes these keys:

| Key      | Description                                                              |
| ---      | -----------                                                              |
| `name`   | The name of the tree-sitter grammar                                      |
| `source` | The method of fetching the grammar - a table with a schema defined below |

Where `source` is a table with either these keys when using a grammar from a
git repository:

| Key    | Description                                               |
| ---    | -----------                                               |
| `git`  | A git remote URL from which the grammar should be cloned  |
| `rev`  | The revision (commit hash or tag) which should be fetched |
| `subpath` | A path within the grammar directory which should be built. Some grammar repositories host multiple grammars (for example `tree-sitter-typescript` and `tree-sitter-ocaml`) in subdirectories. This key is used to point `ms --grammar build` to the correct path for compilation. When omitted, the root of repository is used |

For a grammar already available locally, use `source = { path = "..." }`
instead. Local sources are built but are not fetched by `ms --grammar fetch`.

### Choosing grammars

You may use a top-level `use-grammars` key to control which grammars are
fetched and built when using `ms --grammar fetch` and `ms --grammar build`.

```toml
# Note: this key must come **before** the [[language]] and [[grammar]] sections
use-grammars = { only = [ "rust", "c", "cpp" ] }
```

Or exclude specific grammars instead:

```toml
use-grammars = { except = [ "yaml", "json" ] }
```

When omitted, all grammars are fetched and built.

## Packer templates

Files ending in `.pkr.hcl` or `.pkrvars.hcl` use the Packer configuration.
Install [Packer](https://developer.hashicorp.com/packer/install) and put `packer`
on `PATH` to format these files with `:format` or on save.

Run `ms --health packer` to check formatter availability.

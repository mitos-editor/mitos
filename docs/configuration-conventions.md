# Configuration conventions

These conventions apply to user-facing TOML, Rust configuration types, runtime
option commands, and the configuration reference in `book/src/configuration.md`.

## Organize by feature

Use stable feature tables: `[editor.lsp.messages]`, `[editor.completion.word]`,
`[editor.files.save]`, and `[editor.diagnostics.inline]`. Add a subsection when
it names a feature or groups related settings. Avoid generic buckets such as
`options` or `advanced` and unnecessary nesting around independent values.

Give each setting one owner. Completion menu behavior belongs to
`editor.completion`; LSP snippet support belongs to `editor.lsp.completion`.
Formatting belongs to `editor.format`, even when triggered by saving.

Keep `theme`, `keys`, and `commands` at the top level. Shared editor behavior
belongs under `editor`; terminal capabilities, keyboard protocol, and external
terminal launching belong under the top-level `terminal` table. The frontend
passes its selected launcher to the shared editor for DAP requests.

## Names and value shapes

Use kebab-case for TOML keys and snake_case for Rust fields. Within a section,
omit redundant prefixes: `completion.trigger-length`, not
`completion.completion-trigger-length`.

Use `enable` to enable a feature, `auto-trigger` for automatic invocation, and
`show-*` for presentation options within an enabled feature. Use `on-save` for
formatting on save and `show` for visibility policies such as bufferline's
`never`, `always`, and `multiple`.

Features should have a table from the beginning, even when they have only an
`enable` field. In particular, write `rainbow-brackets.enable = true`,
`auto-pairs.enable = false`, and `bufferline.show = "always"`. Keep boolean,
array, or string shorthands only as explicitly supported legacy input.
`languages.toml` retains its existing value shapes for now, including
`auto-format` and the boolean or character-map `auto-pairs` setting.

Name timings by behavior: `trigger-delay` for deferred completion requests,
`interval` for periodic polling, and `timeout` for an operation's deadline.
Retained legacy names, such as delayed autosave's `timeout`, are documented in
the reference. Integer editor durations are milliseconds; language-server
request timeouts are seconds. Always document the unit, default, and bounds.

## Types, defaults, and validation

Represent feature tables with typed Rust structs. Supply defaults at the
appropriate scope, use `rename_all = "kebab-case"`, and reject unknown fields.
Keep validation errors visible; a misspelling must not silently use a default.

Omitting a feature table keeps its defaults. Omitted leaves in language
feature overrides inherit editor settings rather than substituting a whole
new set of defaults. Reuse shared types when their defaults and semantics are
identical; use optional leaves for language overrides.

Delimiter-pair maps retain their existing replacement behavior; global pairing
disablement takes precedence over language pair maps.

Changing the schema should preserve existing behavior and default values.
Do not add speculative settings solely to fill out a new subsection.

## Layering and compatibility

The order is built-in defaults, user configuration, then trusted project
configuration. Normalize legacy paths independently in each `config.toml` input
**before** merging layers. Reject duplicate logical leaves within a source, including
old and canonical spellings of the same setting. Disjoint legacy and canonical
subsections may coexist during migration.

Merge tables recursively without a fixed depth limit. Ordinary arrays replace
the earlier array, including an explicit empty array. Only top-level language
and grammar definitions merge by `name`; keymaps and commands retain their
own documented merge policies. A grammar
`source` table replaces its predecessor as a unit, so changing between local
and Git sources cannot retain fields from the previous variant.

Language-specific spelling words and ignored-token expressions extend the
editor spelling lists. This is feature-specific inheritance, not a generic
rule to concatenate arrays between configuration files.

Workspace trust is user-scope only. A project's configuration cannot modify
the policy used to decide whether that project's configuration is loaded.

Runtime `:get`, `:set`, and `:toggle` use canonical paths relative to `editor`,
for example `lsp.inlay-hints.enable`. Frontend settings use `terminal.*` paths.
Legacy aliases resolve to the same leaves; completions show canonical paths.

Normalize known renames in memory on startup and configuration reload; loading must
not rewrite files. Offer separate explicit user and trusted workspace migration
commands using the existing selection dialog, with Cancel selected initially.
Provide a CLI export to stdout for generated and read-only configuration.
Check the user-scope trust policy before reading or rewriting project files. Keep
compatibility loading and file rewriting on the same normalization rules.

Validate all loaded layers before writing. Preserve user values, comments,
keybindings, and unrelated sections rather than serializing defaults. Create a
backup without overwriting existing backups, replace files atomically, and preserve
symlinks and permissions. Repeated migration must leave canonical files unchanged.
Respect read-only file permissions, even when atomic replacement would be possible.
Report changed files and backup paths. Explicit persistence failures must warn while
keeping the successfully loaded configuration usable through compatibility aliases.

Preserve existing explicit trust only when reconstructing the approved fingerprint
from the exact migration bytes and its new backup succeeds. Concurrent or unrelated
changes must remain subject to renewed trust; migration must not create new grants.
Do not migrate language files or rewrite files read by health checks, help, or
version output. Missing user configuration must not be created as a side effect.

## Language configuration and documentation

Keep language identity, detection, grammar sources, server process definitions,
and adapter-specific settings in `languages.toml`. The language-file schema
remains unchanged for now: use `language.auto-format`, `language.path-completion`,
`language.word-completion`, and the other established keys. Map these overrides
to the grouped editor settings internally. Future language-schema changes should
use matching feature paths, but must be proposed and migrated separately.
Do not rewrite opaque language-server `config` objects.

Update the complete configuration reference and affected examples with each
schema change. Document compatibility aliases separately from canonical keys.
Test meaningful boundaries: defaults, partial nested overrides, legacy/canonical
precedence, duplicate and unknown keys, array replacement, and runtime updates.

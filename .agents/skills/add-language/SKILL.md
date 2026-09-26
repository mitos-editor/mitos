---
name: add-language
description: Add or extend built-in language support in Mitos, including configuration, Tree-sitter grammars, queries, fixtures, and generated documentation.
---

# Add a Language to Mitos

Read [Adding languages](../../../book/src/guides/adding_languages.md) and
[Languages](../../../book/src/languages.md). Work from the repository root and
follow a comparable entry in `languages.toml` and `runtime/queries/`.

## Configure

- Add a `[[language]]` entry to `languages.toml`; check file-type conflicts
  (globs take precedence over extensions). See the
  [configuration reference](../../../book/src/configuration.md#languagestoml-reference)
  for accepted keys.
- Reuse or define `[language-server.<server>]` and reference it in
  `language-servers`. Verify commands and options against server documentation;
  set `language-id` when its LSP identifier differs from the language name.
- Add a `[[grammar]]` with `source.git`, pinned `source.rev`, and `source.subpath`
  if needed. Reuse an existing grammar via the language's `grammar` key.
  Replace experimental `source.path` values with Git sources before delivery.
  If no grammar or server exists, implement the available support and report the gap.

## Write Queries

For grammar-backed support, add `runtime/queries/<language-name>/highlights.scm`.
Add optional queries as needed. Read the relevant guides:

| Query | Guide |
| --- | --- |
| `highlights.scm` | [Highlights](../../../book/src/guides/highlights.md), [theme scopes](../../../book/src/themes.md) |
| `indents.scm` | [Indentation](../../../book/src/guides/indent.md) |
| `injections.scm` | [Injections](../../../book/src/guides/injection.md) |
| `textobjects.scm` | [Textobjects](../../../book/src/guides/textobject.md) |
| `locals.scm` | [Locals](../../../book/src/guides/locals.md) |
| `tags.scm` | [Tags](../../../book/src/guides/tags.md) |
| `rainbows.scm` | [Rainbow brackets](../../../book/src/guides/rainbow_bracket_queries.md) |

Inspect the pinned grammar's node types or use `:tree-sitter-subtree` to inspect
syntax. Adapt upstream queries to Mitos's captures and predicates. Highlights
should capture the intended leaf: the last match wins for identical spans,
while innermost captures win for nested nodes. Queries may start with
`; inherits: <language>`; changes to shared queries require checking inheriting grammars.

## Validate

Use a binary built from the checkout and its runtime:

```sh
export MITOS_RUNTIME="$PWD/runtime"
ms --grammar fetch
ms --grammar build
```

Add highlight fixtures with caret assertions under
`tests/query/highlights/<language-id>/<name>.<ext>`. For indentation, add
`tests/indent/<language-id>.<ext>`; this tests both reindentation and newlines.
When testing interactively, use `:set indent-heuristic tree-sitter` so hybrid
indentation does not mask errors.

Substitute the configured language ID below; run `indent-check` when adding indentation:

```sh
cargo xtask query-check <language-id>
cargo xtask highlight-check <language-id>
cargo xtask indent-check <language-id>
cargo xtask docgen
```

Ensure filters and fixtures actually exercise the language; query compilation
alone does not verify behavior. Diagnose highlights with
`cargo xtask highlight-check --dump <language-id> <file>`. Smoke-test file
detection and added editor features where available, and report validation gaps.

Review generated changes to `book/src/lang-support.md`; `docgen` also rewrites
command documentation, so preserve unrelated work. Include new server setup
instructions locally; publishing to the upstream wiki requires a separate request.

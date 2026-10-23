# Adding folding queries

Add `runtime/queries/<language>/folds.scm` with `@fold` captures on syntax
regions that should collapse. For example, JSON folds objects and arrays:

```scheme
[(object) (array)] @fold
```

Mitos keeps each captured region's first line. When the captured node has paired
`{}`, `[]`, or `()` syntax tokens, its closing token and the rest of that source
line remain visible after the fold placeholder. The syntax tree identifies the
tokens; delimiter-like text inside strings and comments is not used as a closer.

Other captures conceal whole lines, including the last captured line. A capture
ending at the start of a line excludes that line. Single-line captures are ignored.
Regions must be nested or disjoint; crossing regions are ignored. Injected
languages use their own folding queries.

Queries support the standard Tree-sitter text predicates and query inheritance
with `; inherits: <language>`. Folding uses only `@fold` captures; editor-specific
range directives such as `#trim!`, `@start`, and `@end` are not supported.

Inspect the pinned grammar's node types, then run:

```sh
cargo xtask query-check <language>
cargo xtask docgen
```

Also test folding real source in the editor: query compilation checks node
names and predicates, but does not establish that the intended region folds.

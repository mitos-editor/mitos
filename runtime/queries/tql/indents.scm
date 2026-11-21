;; Upstream: https://github.com/tenzir/tree-sitter-tql/blob/main/queries/tql/indents.scm

[
  (then_block)
  (else_block)
  (pipeline_block)
  (pipeline_expression)
  (match_statement)
  (match_arm)
  (list)
  (record)
] @indent

[
  "}"
  "]"
] @outdent

[
  (string)
  (format_text)
  (comment)
  (frontmatter_body)
] @opaque

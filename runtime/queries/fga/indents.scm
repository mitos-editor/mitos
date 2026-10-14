(source_file
  (model)
  (schema) @indent
  (#set! "scope" "header"))

(condition_declaration
  (param) @indent
  (#set! "scope" "header"))

[
  (contents)
  (type_declaration)
  (relations)
  (definition)
  (direct_relationship)
  (parenthesized_relation)
  (condition_body)
  (parenthesized_condition)
  (bracket_condition)
  (braced_condition)
  (container_type_identifier)
] @indent

["}" "]" ")" ">"] @outdent

[(string) (bytes) (comment)] @opaque

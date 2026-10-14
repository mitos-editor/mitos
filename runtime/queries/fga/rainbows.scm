[
  (direct_relationship)
  (parenthesized_relation)
  (condition_declaration)
  (condition_body)
  (parenthesized_condition)
  (bracket_condition)
  (braced_condition)
  (container_type_identifier)
] @rainbow.scope

["[" "]" "(" ")" "{" "}"] @rainbow.bracket

(container_type_identifier ["<" ">"] @rainbow.bracket)

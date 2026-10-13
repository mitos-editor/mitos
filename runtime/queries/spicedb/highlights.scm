; highlights.scm

[
  "definition"
  "caveat"
  "partial"
  "permission"
  "relation"
  "nil"
] @keyword

[
  ","
  ":"
] @punctuation.delimiter

[
  "("
  ")"
  "{"
  "}"
] @punctuation.bracket

[
  "|"
  "+"
  "-"
  "&"
  "#"
  "->"
  "="
] @operator
["with" "and"] @keyword.operator

[
  "nil"
  "*"
] @constant.builtin

(comment) @comment
(type_identifier) @type
(cel_type_identifier) @type
(cel_variable_identifier) @variable.parameter
(field_identifier) @variable.other.member
(func_identifier (identifier) @function)
(method_identifier) @function.method

["use" "import"] @keyword.control.import
(use_flag name: (identifier) @keyword.directive)
"expiration" @keyword.storage.modifier
(self_expression) @variable.builtin
(import path: (string_literal) @string.special.path)
["." ";"] @punctuation.delimiter
["..." "any" "all"] @operator

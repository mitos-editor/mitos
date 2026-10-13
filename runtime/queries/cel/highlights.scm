; Operators

[
  "-"
  "!"
  "*"
  "/"
  "&&"
  "%"
  "+"
  "<"
  "<="
  "=="
  ">"
  ">="
  "||"
] @operator

; Keywords

[
"in"
] @keyword.operator

; Identifiers

(identifier) @variable
(reserved_keyword) @keyword

(select_expression
  member: [(identifier) (reserved_keyword)] @variable.other.member)

(field_initializer
  key: [(identifier) (reserved_keyword)] @variable.other.member)

; Function calls

(call_expression
  function: (identifier) @function)

(member_call_expression
  function: (identifier) @function.method)

; Literals

[
  (bytes_literal)
  (double_quote_string_literal)
  (single_quoted_string_literal)
  (triple_double_quote_string_literal)
  (triple_single_quoted_string_literal)
] @string

[
  (int_literal)
  (uint_literal)
] @constant.numeric.integer
(float_literal) @constant.numeric.float

[
  (true)
  (false)
] @constant.builtin.boolean

(null) @constant.builtin

(comment) @comment

(struct_expression type: (identifier) @type)
(struct_expression type: (select_expression member: (identifier) @type))

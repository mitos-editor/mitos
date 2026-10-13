
(type_identifier) @type
((type_identifier) @type.builtin (#any-of? @type.builtin "int" "bool" "cell" "slice" "builder" "continuation" "tuple" "coins" "address" "void" "self"))

(identifier) @variable
((identifier) @constant
  (#match? @constant "^[A-Z][A-Z\\d_]*$"))
((identifier) @variable.builtin
 (#eq? @variable.builtin "self"))

(comment) @comment

[
  "(" ")"
  "{" "}"
] @punctuation.bracket

[
  ; "::"
  "."
  ";"
  ","
  ":"
] @punctuation.delimiter

[
  "do"
  "if"
  "as"
  "fun"
  "asm"
  "get"
  "try"
  "var"
  "val"
  "else"
  "true"
  "tolk"
  "const"
  "false"
  "throw"
  "redef"
  "while"
  "catch"
  "return"
  "assert"
  "import"
  "global"
  "repeat"
  "mutate"
  "struct"
  "enum"
  "contract"
  "type"
  "match"
  "lazy"
  (builtin_specifier)
] @keyword

[
  "="
  "+="
  "-="
  "*="
  "/="
  "%="
  "<<="
  ">>="
  "&="
  "|="
  "^="

  "=="
  "<"
  ">"
  "<="
  ">="
  "!="
  "<=>"
  "<<"
  ">>"
  "~>>"
  "^>>"
  "-"
  "+"
  "|"
  "^"
  "*"
  "/"
  "%"
  "~/"
  "^/"
  "&"
  "~"
  "."
  "!"
  "&&"
  "||"

  "->"
  "=>"
] @operator

(string_literal) @string
(number_literal) @constant.numeric
(boolean_literal) @constant.builtin.boolean

(annotation) @attribute
(annotation_name) @attribute
(null_literal) @constant.builtin

(function_declaration
  name: (identifier) @function)
(method_declaration
  name: (identifier) @function)
(get_method_declaration
  name: (identifier) @function)
(dot_access field: (identifier) @variable.other.member)
(function_call
  callee: (identifier) @function)
(function_call
  callee: (dot_access field: (identifier) @function.method))



(struct_declaration
  "struct"
  name: (identifier) @type)
(struct_field_declaration
  name: (identifier) @variable.other.member)
(instance_argument
  name: (identifier) @variable.other.member)

(enum_declaration name: (identifier) @type.enum)
(contract_declaration name: (identifier) @type)
(enum_member_declaration name: (identifier) @type.enum.variant)

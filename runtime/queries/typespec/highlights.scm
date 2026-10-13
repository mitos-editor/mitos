; Keywords

[
  "is"
  "extends"
  "valueof"
] @keyword.operator

[
  "namespace"
  "scalar"
  "interface"
  "alias"
] @keyword

[
  "model"
  "enum"
  "union"
] @keyword.storage.type

[
  "op"
  "fn"
  "dec"
] @keyword.function

"extern" @keyword.storage.modifier

[
  "import"
  "using"
] @keyword.control.import

[
  "("
  ")"
  "{"
  "}"
  "<"
  ">"
  "["
  "]"
] @punctuation.bracket

[
  ","
  ";"
  "."
  ":"
] @punctuation.delimiter

[
  "|"
  "&"
  "="
  "..."
] @operator

"?" @punctuation.special

; Identifiers

(identifier_or_member_expression (identifier) @type)
(member_expression base: (identifier) @namespace member: (identifier) @type)

; Namespaces

(using_statement
  module: (identifier_or_member_expression [(identifier) @namespace (member_expression (identifier) @namespace)]))

(namespace_statement
  name: (identifier_or_member_expression [(identifier) @namespace (member_expression (identifier) @namespace)]))

; Comments

[
  (single_line_comment)
] @comment.line

[
  (multi_line_comment)
] @comment.block

; Decorators

(decorator
  "@" @attribute
  name: (identifier_or_member_expression [(identifier) @attribute (member_expression member: (identifier) @attribute .)]))

(augment_decorator_statement
  name: (identifier_or_member_expression [(identifier) @attribute (member_expression member: (identifier) @attribute .)]))


; Scalars

(scalar_statement
  name: (identifier) @type)

; Models

(model_statement
  name: (identifier) @type)

(model_property
  name: (identifier) @variable.other.member)

; Operations

(operation_statement
  name: (identifier) @function)

(operation_arguments
  (model_property
    name: (identifier) @variable.parameter))

(template_parameter
  name: (identifier) @type.parameter)

(function_parameter
  name: (identifier) @variable.parameter)

; Interfaces

(interface_statement
  name: (identifier) @type)

(interface_statement
  (interface_body
    (interface_member
      (identifier) @function.method)))

; Enums

(enum_statement
  name: (identifier) @type.enum)

(enum_member
  name: (identifier) @constant)

; Unions

(union_statement
  name: (identifier) @type)

(union_variant
  name: (identifier) @type.enum.variant)

; Aliases

(alias_statement
  name: (identifier) @type)

; Built-in types

[
  (quoted_string_literal)
  (triple_quoted_string_literal)
] @string

(escape_sequence) @constant.character.escape

(boolean_literal) @constant.builtin.boolean

[
  (hex_integer_literal)
  (binary_integer_literal)
] @constant.numeric.integer

(builtin_type) @type.builtin

(decimal_literal) @constant.numeric
"const" @keyword.storage.modifier
"typeof" @keyword.operator
(const_statement name: (identifier) @constant)
(object_member key: (identifier) @variable.other.member)
(directive "#" @keyword.directive name: (identifier_or_member_expression (identifier) @keyword.directive))

; Paths override the general string rule on the same span.
(import_statement (quoted_string_literal) @string.special.path)

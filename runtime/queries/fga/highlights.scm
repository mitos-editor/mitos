(extended_identifier) @variable.other.member

(module
  name: (identifier) @namespace)

(condition_declaration
  name: (identifier) @function)

(condition_declaration
  (param
    name: (identifier) @variable.parameter))

(conditional
  condition: (identifier) @function)

(inline_condition) @constant.builtin

(type_declaration
  name: (extended_identifier) @type)

(direct_relationship_item
  (extended_identifier) @type)

(relation_ref) @type
(all) @type

((simple_type_identifier) @type.builtin)

(container_type_identifier ["map" "list"] @type.builtin)

(version) @constant.numeric
(int) @constant.numeric.integer
(uint) @constant.numeric.integer
(float) @constant.numeric.float

(string) @string
(bytes) @string.special
(file) @string.special.path

(boolean) @constant.builtin.boolean
(null) @constant.builtin

(condition_body
  (identifier) @variable)

(parenthesized_condition
  (identifier) @variable)

(bracket_condition
  (identifier) @variable)

(braced_condition
  (identifier) @variable)

; CEL identifiers are flat children of the surrounding condition node.
(_ "." . (identifier) @variable.other.member)
(_ (identifier) @function . (parenthesized_condition))
(_ "." . (identifier) @function.method . (parenthesized_condition))

(operator) @operator
(condition_operator) @operator

(direct_relationship ["[" "]"] @punctuation.bracket)
(parenthesized_relation ["(" ")"] @punctuation.bracket)
(condition_declaration ["(" ")"] @punctuation.bracket)
(condition_body ["{" "}"] @punctuation.bracket)
(parenthesized_condition ["(" ")"] @punctuation.bracket)
(bracket_condition ["[" "]"] @punctuation.bracket)
(braced_condition ["{" "}"] @punctuation.bracket)
(container_type_identifier ["<" ">"] @punctuation.bracket)

[":" "," "."] @punctuation.delimiter

(model) @keyword
(module "module" @keyword)
(schema "schema" @keyword)
(contents "contents" @keyword)
(relations "relations" @keyword)
(type_declaration "extend" @keyword)
(type_declaration "type" @keyword)
(definition "define" @keyword)

(indirect_relation "from" @keyword.operator)
(conditional "with" @keyword.operator)
(condition_declaration "condition" @keyword.function)

(comment) @comment

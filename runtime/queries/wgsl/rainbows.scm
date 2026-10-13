[
  (struct_declaration)
  (function_declaration)
  (attribute)
  (type_declaration)
  (compound_statement)
  (argument_list_expression)
  (postfix_expression)
  (variable_qualifier)
  (subscript_expression)
  (parenthesized_expression)
] @rainbow.scope

["(" ")" "[" "]" "{" "}"] @rainbow.bracket

(type_declaration ["<" ">"] @rainbow.bracket) @rainbow.scope
(variable_qualifier ["<" ">"] @rainbow.bracket) @rainbow.scope

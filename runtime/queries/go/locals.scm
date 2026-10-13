; Scopes

[
  (function_declaration)
  (method_declaration)
  (func_literal)
  (type_spec)
  (type_alias)
  (block)
  (if_statement)
  (for_statement)
  (expression_switch_statement)
  (type_switch_statement)
  (select_statement)
  (expression_case)
  (type_case)
  (communication_case)
  (default_case)
] @local.scope

; Definitions

(parameter_declaration (identifier) @local.definition.variable.parameter)
(variadic_parameter_declaration (identifier) @local.definition.variable.parameter)

(type_parameter_declaration
 name: (identifier) @local.definition.type.parameter)

; Generic receivers declare their own type parameters.
(method_declaration
 receiver: (parameter_list
  (parameter_declaration
   type: [
    (generic_type
     type_arguments: (type_arguments
      (type_elem (type_identifier) @local.definition.type.parameter)))
    (pointer_type
     (generic_type
      type_arguments: (type_arguments
       (type_elem (type_identifier) @local.definition.type.parameter))))
   ])))

(const_declaration
 (const_spec
  name: (identifier) @local.definition.constant))

(var_spec
 name: (identifier) @local.definition.variable)

(short_var_declaration
 left: (expression_list (identifier) @local.definition.variable))

(range_clause
 left: (expression_list (identifier) @local.definition.variable)
 ":=")

(receive_statement
 left: (expression_list (identifier) @local.definition.variable)
 ":=")

; Bound identifier of `switch v := x.(type)`.
(type_switch_statement
 alias: (expression_list (identifier) @local.definition.variable))

; References

(identifier) @local.reference
(type_identifier) @local.reference

; Field names in struct literals are identifier rather than field_identifier,
; these cannot be locals.
(keyed_element . (literal_element (identifier) @_))

; A declaration name is not a reference to one of its type parameters.
(type_spec name: (type_identifier) @_)
(type_alias name: (type_identifier) @_)

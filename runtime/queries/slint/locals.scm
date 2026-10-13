[
  (component)
  (component_definition)
  (function_definition)
  (closure_expression)
  (imperative_block)
] @local.scope

(closure_expression argument: (simple_identifier) @local.definition.variable.parameter)
(typed_identifier name: (simple_identifier) @local.definition.variable.parameter)
(simple_identifier) @local.reference
(function_call name: (simple_identifier) @_)
(property_assignment property: (simple_identifier) @_)

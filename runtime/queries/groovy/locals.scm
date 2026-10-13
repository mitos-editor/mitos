(function_definition) @local.scope

(parameter
  name: (identifier) @local.definition.variable.parameter)

(identifier) @local.reference

; Member names and named argument labels do not resolve as locals.
(dotted_identifier (identifier) @_ .)
(map_item key: (identifier) @_)

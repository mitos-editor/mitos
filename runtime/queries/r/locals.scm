; locals.scm

(function_definition) @local.scope

(parameters (parameter name: (identifier) @local.definition.variable.parameter))

(identifier) @local.reference

; Member names and argument labels are not local references.
(extract_operator rhs: (identifier) @_)
(namespace_operator rhs: (identifier) @_)
(argument name: (identifier) @_)

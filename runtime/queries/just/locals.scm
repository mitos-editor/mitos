; This file tells us about the scope of variables so e.g. local
; variables override global functions with the same name

; Scope

(recipe) @local.scope
(function) @local.scope

; Definitions

(alias
  alias_name: (identifier) @local.definition.function)

(assignment
  name: (identifier) @local.definition.variable)

(function
  name: (identifier) @local.definition.function)

(mod
  name: (identifier) @local.definition.namespace)

(recipe_parameter
  name: (identifier) @local.definition.variable.parameter)

(recipe
  name: (identifier) @local.definition.function)

(function_decl_parameters
  (identifier) @local.definition.variable.parameter)

; References

(alias
  name: (identifier) @local.reference)

(function_call
  name: (identifier) @local.reference)

(module_path
  name: (identifier) @local.reference)

(recipe_dependency
  name: (identifier) @local.reference)

(value
  (identifier) @local.reference)

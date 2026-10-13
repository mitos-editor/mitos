; Scopes       @local.scope
; -------------------------

[
  (global_function)
  (init_function)
  (bounced_function)
  (receive_function)
  (external_function)
  (storage_function)
  (block_statement)
  (foreach_statement)
  (catch_clause)
] @local.scope

; Definitions  @local.definition
; ------------------------------

(parameter
  name: (identifier) @local.definition.variable.parameter)

(storage_constant
  name: (identifier) @local.definition.constant)

; References   @local.reference
; -----------------------------

(self) @local.reference

(identifier) @local.reference


(let_statement name: (identifier) @local.definition.variable)
(global_constant name: (identifier) @local.definition.constant)
(foreach_statement key: (identifier) @local.definition.variable
  value: (identifier) @local.definition.variable)
(catch_clause name: (identifier) @local.definition.variable)
(destruct_bind !bind name: (identifier) @local.definition.variable)
(destruct_bind bind: (identifier) @local.definition.variable)
(field_access_expression name: (identifier) @_)
(instance_argument name: (identifier) @_)
(destruct_bind name: (identifier) @_ bind: (_))

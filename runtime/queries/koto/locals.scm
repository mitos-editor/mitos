; Scopes
(module) @local.scope

(function) @local.scope

; Definitions
(arg
  (identifier) @local.definition.variable.parameter)

(arg
  (variable (identifier) @local.definition.variable.parameter))

(import_item
  (identifier) @local.definition.namespace)

(entry_block
  key: (identifier) @local.definition.variable.other.member)

(entry_inline
  key: (identifier) @local.definition.variable.other.member)

; References
(identifier) @local.reference

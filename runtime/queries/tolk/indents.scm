; indent
; ------

[
  (block_statement)
  (match_expression)
  (struct_declaration)
  (enum_declaration)
  (contract_declaration)
  (object_literal)
] @indent

; outdent
; -------

[
  "}"
  ")"
] @outdent

; indent.always
; outdent.always
; align
; extend
; extend.prevent-once

(type_parameters ">" @outdent)

; D2 diagram nodes are definitions; connection declarations are not.
(declaration
  .
  [(identifier) (identifier_chain)] @name
  .
  [(label) (block)]) @definition.class

; A bare node declaration has just its name.
(declaration
  .
  [(identifier) (identifier_chain)] @name
  .) @definition.class

(method_declaration
  (identifier) @name) @definition.function

; Functions
(private_function
  (function_signature
    (identifier) @name)) @definition.function

(read_only_function
  (function_signature
    (identifier) @name)) @definition.function

(public_function
  (function_signature
    (identifier) @name)) @definition.function

(trait_definition
  "define-trait" . (identifier) @name) @definition.interface

(constant_definition
  "define-constant" . (identifier) @name) @definition.constant

(variable_definition
  "define-data-var" . (identifier) @name) @definition.constant

(mapping_definition
  "define-map" . (identifier) @name) @definition.struct

(fungible_token_definition
  "define-fungible-token" . (identifier) @name) @definition.type

(non_fungible_token_definition
  "define-non-fungible-token" . (identifier) @name) @definition.type

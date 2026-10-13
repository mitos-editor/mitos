(struct_declaration
  name: (identifier) @name) @definition.struct

(function_declaration
  name: (identifier) @name) @definition.function

(global_variable_declaration
  (variable_declaration
    (variable_identifier_declaration
      name: (identifier) @name))) @definition.constant

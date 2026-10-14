(module
  name: (identifier) @name) @definition.module

(type_declaration
  name: (extended_identifier) @name) @definition.type

(definition
  relation: (extended_identifier) @name) @definition.field

(condition_declaration
  name: (identifier) @name) @definition.function

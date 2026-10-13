(object_definition
  name: (type_identifier) @name) @definition.type

(type_identifier) @name @reference.type

(partial_definition name: (type_identifier) @name) @definition.type
(caveat_definition name: (func_identifier) @name) @definition.function
(relation name: (field_identifier) @name) @definition.field
(permission name: (method_identifier) @name) @definition.function

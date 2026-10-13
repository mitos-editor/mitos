(class_declaration
  (type_identifier) @name) @definition.class

(object_declaration
  "object" (type_identifier) @name) @definition.class

(function_declaration
  (simple_identifier) @name) @definition.function

(property_declaration
  (variable_declaration
    (simple_identifier) @name)) @definition.constant

; Functions and declarations expose a leaf name and their enclosing symbol.
(function_definition
  declarator: (function_declarator
    declarator: [(identifier) (field_identifier)] @name)) @definition.function

(declaration
  declarator: (function_declarator
    declarator: [(identifier) (field_identifier)] @name)) @definition.function

(function_definition
  declarator: (function_declarator
    declarator: (qualified_identifier name: (identifier) @name))) @definition.function

(preproc_function_def name: (identifier) @name) @definition.macro
(preproc_def name: (identifier) @name) @definition.constant
(type_definition declarator: (type_identifier) @name) @definition.type
(struct_specifier name: (type_identifier) @name) @definition.struct
(enum_specifier name: (type_identifier) @name) @definition.enum
(union_specifier name: (type_identifier) @name) @definition.struct
(class_specifier name: (type_identifier) @name) @definition.class
(namespace_definition name: (namespace_identifier) @name) @definition.module
(concept_definition name: (identifier) @name) @definition.interface
(alias_declaration name: (type_identifier) @name) @definition.type

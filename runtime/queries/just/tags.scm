; Symbols that can be considered definitions in a Just file.

(alias
  alias_name: (identifier) @name) @definition.function

(assignment
  name: (identifier) @name) @definition.constant

(function
  name: (identifier) @name) @definition.function

(import
  (path) @name) @definition.module

(mod
  name: (identifier) @name) @definition.module

(recipe
  name: (identifier) @name) @definition.function

(unexport
  name: (identifier) @name) @definition.constant

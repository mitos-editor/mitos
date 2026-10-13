(section
  (identifier) @_type
  (attribute
    (identifier) @_attr_type
    (string) @name)
  (#eq? @_type "node")
  (#eq? @_attr_type "name")) @definition.struct

(section
  (identifier) @name
  (#not-eq? @name "node")) @definition.section

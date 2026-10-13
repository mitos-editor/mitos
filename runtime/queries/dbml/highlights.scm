; Adapted from dynamotn/tree-sitter-dbml queries/highlights.scm.

(identifier) @variable

; Keywords

[
  "Project"
  "Table"
  "TablePartial"
  "TableGroup"
  "Enum"
  "Ref"
  "Note"
  "indexes"
  "checks"
  "records"
  "DiagramView"
  "Tables"
  "Notes"
  "TableGroups"
  "Schemas"
  "Metadata"
  "Dep"
  "as"
] @keyword

[
  "use"
  "reuse"
  "from"
] @keyword.control.import

(element_kind) @keyword

; Definitions

(table_definition
  name: (qualified_name
    (identifier) @type .))

(table_partial_definition
  name: (identifier) @type)

(table_group_definition
  name: (identifier) @type)

(enum_definition
  name: (qualified_name
    (identifier) @type .))

(alias
  (identifier) @type)

(partial_injection
  name: (identifier) @type)

(column_definition
  name: (identifier) @variable.other.member)

(column_type
  name: (identifier) @type)

(enum_value
  name: (identifier) @constant)

(note_definition
  name: (identifier) @label)

(ref_definition
  name: (identifier) @label)

(dep_definition
  name: (identifier) @label)

(diagram_view_definition
  name: (identifier) @label)

(ref_endpoint
  (identifier) @variable.other.member .)

; Settings and properties

(setting_name) @attribute

(inline_ref
  "Ref" @attribute)

(inline_dep
  "Dep" @attribute)

(property
  key: (identifier) @variable.other.member)

; Literals

(string) @string

(number) @constant.numeric

(boolean) @constant.builtin.boolean

(null) @constant.builtin

(color) @string.special

(expression) @string.special

(wildcard) @operator

(comment) @comment

; Operators and punctuation

[
  (relation)
  (arrow)
  "~"
] @operator

[
  "("
  ")"
  "["
  "]"
  "[]"
  "{"
  "}"
] @punctuation.bracket

[
  ","
  "."
  ":"
] @punctuation.delimiter

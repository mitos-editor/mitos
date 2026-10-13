; Keywords
[
  "module"
  "using"
  "single"
  "multi"
  "link"
  "property"
  "constraint"
  "tuple"
  "annotation"
  "abstract"
  "scalar"
  "type"
  "required"
  "optional"
  "extension"
  "function"
] @keyword

(identifier) @variable

(modifier) @keyword
"extending" @keyword

(module name: (identifier) @namespace)
(object_type name: (identifier) @type)

(comment) @comment

; Properties (includes both property and link declarations, since link keyword is optional)
(property name: (identifier) @variable.other.member)

(string) @string
(edgeql_fragment) @string

; Builtins
(type (identifier) @type)
[
  "str"
  "bool"
  "int16"
  "int32"
  "int64"
  "float32"
  "float64"
  "bigint"
  "decimal"
  "json"
  "uuid"
  "bytes"
  "datetime"
  "duration"
  "sequence"
  "anytype"
] @type.builtin

(true) @constant.builtin.boolean
(false) @constant.builtin.boolean
(null) @constant.builtin

; Delimiters
[
  ";"
  ","
] @punctuation.delimiter

; Operators
[
  "->"
  ":="
] @operator

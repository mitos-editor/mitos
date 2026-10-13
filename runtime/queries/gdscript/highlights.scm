(identifier) @variable

; Identifier naming conventions

(
  (identifier) @constant 
  (#match? @constant "^[A-Z][A-Z\\d_]+$"))

; class
(class_name_statement (name) @type)
(class_definition (name) @type)


; Function calls

(attribute_call (identifier) @function.method)
(base_call (identifier) @function)
(call (identifier) @function)
(lambda (name) @function)

; Function definitions

(function_definition 
  name: (name) @function
  parameters: (parameters) )
(constructor_definition "_init" @function)



;; Literals
(comment) @comment
(string) @string

(type (identifier) @type)
(enum_definition (name) @type.enum)
(enumerator (identifier) @type.enum.variant)
((identifier) @variable.builtin
 (#eq? @variable.builtin "_"))


(variable_statement (identifier) @variable)
(attribute 
  (identifier) 
  (identifier) @variable.other.member)
(attribute 
  (identifier) @type.builtin
  (#match? @type.builtin "^(AABB|Array|Basis|bool|Callable|Color|Dictionary|float|int|NodePath|Object|Packed(Byte|Color|String)Array|PackedFloat(32|64)Array|PackedInt(32|64)Array|PackedVector(2|3)Array|Plane|Projection|Quaternion|Rect2([i]{0,1})|RID|Signal|String|StringName|Transform(2|3)D|Variant|Vector(2|3|4)([i]{0,1}))$"))

[
  (string_name)
  (node_path)
  (get_node)
] @label
(signal_statement (name) @label)

(const_statement (name) @constant)
(integer) @constant.numeric.integer
(float) @constant.numeric.float
(escape_sequence) @constant.character.escape
[
  (true)
  (false)
] @constant.builtin.boolean
(null) @constant.builtin

[
  "+"
  "-"
  "*"
  "/"
  "%"
  "=="
  "!="
  ">"
  "<"
  ">="
  "<="
  "="
  "+="
  "-="
  "*="
  "/="
  "%="
  "&"
  "|"
  "^"
  "~"
  "<<"
  ">>"
  ":="
  ":"
] @operator

(annotation (identifier) @keyword.storage.modifier)

[
  "if"
  "else"
  "elif"
  "match"
  "when"
] @keyword.control.conditional

[
  "while"
  "for"
] @keyword.control.repeat

[
  "return"
  "pass"
  "break"
  "continue"
] @keyword.control.return

[
  "func"
] @keyword.function


[
  "in"
  "is"
  "as"
  "and"
  "or"
  "not"
] @keyword.operator

[
  "var"
  "class"
  "class_name"
  "enum"
] @keyword.storage.type


[
  (static_keyword)
  "const"
  "signal"
  "@"
] @keyword.storage.modifier

[
  "extends"
  "set"
  "get"
  "await"
] @keyword


(parameters (identifier) @variable.parameter)
[(typed_parameter . (identifier) @variable.parameter)
 (default_parameter . (identifier) @variable.parameter)
 (typed_default_parameter . (identifier) @variable.parameter)]
(region_start) @comment
(region_end) @comment
(region_label) @comment

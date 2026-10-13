(string) @string

(enumeral) @constant
(number) @constant.numeric

(variable) @variable
(column_type) @type

(arguments (identifier) @variable.other.member)
(column_declaration . (identifier) @variable.other.member)
(model_declaration (identifier) @type)
(view_declaration (identifier) @type)

[
 "datasource"
 "enum"
 "generator"
 "model"
 "type"
 "view"
] @keyword

[
 (comment)
 (developer_comment)
] @comment

(call_expression . (identifier) @function.builtin)
(attribute (identifier) @attribute)
(block_attribute_declaration (identifier) @attribute)
(attribute (call_expression . (identifier) @attribute))
(block_attribute_declaration (call_expression . (identifier) @attribute))
(attribute (member_expression (identifier) @attribute))

[
 (true)
 (false)
] @constant.builtin.boolean
(null) @constant.builtin

[
 "("
 ")"
 "["
 "]"
 "{"
 "}"
] @punctuation.bracket

[
 ":" 
 ","
] @punctuation.delimiter

[
 "="
 "@"
 "@@"
 (binary_expression)
] @operator

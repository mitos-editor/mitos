[
  (comment)
  (block_comment)
] @comment

[
  (label)
  (label_array)
  (label_codeblock)
] @string

(escape) @constant.character.escape
(identifier) @variable
(argument_name) @variable.parameter
(argument_type) @type
(variable) @variable
(spread_variable) @variable
(import) @string.special.path
(connection) @operator
(connection_identifier) @label
(glob) @string.special
(recursive_glob) @string.special
(global_glob) @string.special
(integer) @constant.numeric
(float) @constant.numeric
["true" "false"] @constant.builtin.boolean
(visibility_mark) @operator

[
  "$"
  "...$"
  "@"
  "...@"
] @keyword

[
  "["
  "]"
  "("
  ")"
  "{"
  "}"
  "|"
  "||"
  "|||"
  "|`"
  "`|"
] @punctuation.bracket

[
  "."
  ","
  ":"
  ";"
] @punctuation.delimiter

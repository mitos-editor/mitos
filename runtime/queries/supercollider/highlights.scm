(line_comment) @comment.line
(block_comment) @comment.block

(argument name: (identifier) @variable.parameter)

(local_var name: (identifier) @variable)
(environment_var name:(identifier) @variable.builtin)
(builtin_var) @constant.builtin

(function_definition name: (variable (_ name: (identifier) @function)))

(named_argument name: (identifier) @variable.other.member)

(function_call name: (identifier) @function)
(function_call "." name: (identifier) @function.method)

(class) @type

(number) @constant.numeric
(float) @constant.numeric.float

(string) @string
(symbol) @string.special.symbol

[
"&&"
"||"
"&"
"|"
"^"
"=="
"!="
"<"
"<="
">"
">="
"<<"
">>"
"+"
"-"
"*"
"/"
"%"
"="
"|@|"
"@@"
"@|@"
] @operator

[
"arg"
"classvar"
"const"
"var"
] @keyword

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
  "|"
] @punctuation.bracket

[
  ";"
  "."
  ","
] @punctuation.delimiter

((function_call name: (identifier) @keyword.control.conditional)
 (#any-of? @keyword.control.conditional "if" "switch" "case"))

((function_call name: (identifier) @keyword.control.repeat)
 (#any-of? @keyword.control.repeat "while" "for" "forBy" "loop"))

(escape_sequence) @constant.character.escape

(duplicated_statement "!" @operator)

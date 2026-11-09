;; Simple tokens
(terminal) @string

(special_sequence) @string.special

(integer) @constant.numeric.integer

(comment) @comment.block

;; Identifiers
(identifier) @variable

;; Punctuation
[
 ";"
 ","
] @punctuation.delimiter

[
 "|"
 "*"
 "-"
] @operator

"=" @keyword.operator

[
 "("
 ")"
 "["
 "]"
 "{"
 "}"
] @punctuation.bracket

; regular escapes like `\n` are detected using another grammar
; Here, we only detect `{{` and `}}` as escapes for `{` and `}`
(escaped) @constant.character.escape

[
  "#"
  (type)
] @special

[
  (sign)
  (fill)
  (align)
  (width)
] @operator

(count) @constant.numeric

(colon) @punctuation.delimiter

(identifier) @variable

; SCREAMING_CASE is assumed to be constant
((identifier) @constant
 (#match? @constant "^[A-Z_]+$"))

[
  "{"
  "}"
] @punctuation.special

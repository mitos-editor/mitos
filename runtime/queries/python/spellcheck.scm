; Docstrings are initial bare strings; expression_statement is now a supertype.
; Comments may precede them. Bytes and f-strings are not documentation.
(module
  . (comment)*
  . (string
      (string_start) @_start
      (string_content) @spell)
  (#not-match? @_start "[bBfF]"))

([
  (class_definition
    body: (block
      . (comment)*
      . (string
          (string_start) @_start
          (string_content) @spell)))
  (function_definition
    body: (block
      . (comment)*
      . (string
          (string_start) @_start
          (string_content) @spell)))
]
  (#not-match? @_start "[bBfF]"))

(escape_sequence) @nospell

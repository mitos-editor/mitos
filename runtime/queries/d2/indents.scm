(block) @indent
"}" @outdent

; Leading whitespace in text blocks and block comments is literal content.
; Embedded languages with indent queries still use their own rules.
[
  (label_codeblock)
  (block_comment)
] @opaque

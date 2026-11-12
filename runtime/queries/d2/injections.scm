; https://d2lang.com/tour/text/#code
; Tagged text blocks select a language (including D2's short aliases such as
; md, py, and tex).
(label_codeblock
  (codeblock_language) @injection.language
  (codeblock_content) @injection.content)

; Untagged D2 text blocks default to Markdown. The opening delimiter is made
; of punctuation and is followed by whitespace when no language is specified.
((label_codeblock
  (codeblock_content) @injection.content) @_codeblock
  (#match? @_codeblock "^[^a-zA-Z0-9_\\s]+\\s")
  (#set! injection.language "markdown"))

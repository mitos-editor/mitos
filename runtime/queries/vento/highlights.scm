(comment) @comment
(front_matter_delimiter) @punctuation.delimiter

; Literal keyword strings - these are the actual text tokens
"if" @keyword.control.conditional
"/if" @keyword.control.conditional
"else" @keyword.control.conditional
"for" @keyword.control.repeat
"/for" @keyword.control.repeat
"await" @keyword.control
"of" @keyword.control

"layout" @keyword.control
"/layout" @keyword.control
"slot" @keyword
"/slot" @keyword
"default" @keyword
"/default" @keyword

"set" @keyword
"/set" @keyword
"echo" @keyword

"import" @keyword.control.import
"from" @keyword.control.import
"export" @keyword.control.import
"/export" @keyword.control.import
"include" @keyword.control.import

"function" @keyword.function
"/function" @keyword.function
"async" @keyword.function

"fragment" @keyword
"/fragment" @keyword

; Tag delimiters
["{{" "{{-" "{{>" "{{->" "{{#" "{{#-"] @punctuation.bracket
["}}" "-}}" "#}}" "-#}}"] @punctuation.bracket

; Operators
("|>") @operator
"=" @operator
"," @punctuation.delimiter

; Identifiers
(identifier) @variable

; Code blocks (disable highlighting to allow injection)
(code) @none

; Filter functions (must come after the general code rule to override it)
(filter (code) @function)

(generic) @variable

(comment) @comment

(block_directive
	(directive (directive_name) @type))

[
	"{"
	"}"
	"("
	")"
	"["
	"]"
] @punctuation.bracket

(simple_directive
	(directive (directive_name) @function))

[
	";"
] @punctuation.delimiter

((generic) @keyword
 (#any-of? @keyword
 	"on"
 	"off"
 	"any"
 	"auto"))

(modifier) @operator


(string) @string

(number) @constant.numeric
(metric) @constant.numeric

(variable) @variable.parameter

(regex) @string

(modifier) @keyword.operator

(lua_block_directive
	"access_by_lua_block" @function)

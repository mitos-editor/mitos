(identifier) @variable

["strict" "graph" "digraph" "subgraph" "node" "edge"] @keyword
(string_literal) @string
(number_literal) @constant.numeric

[
  (edgeop)
  (operator)
] @operator

[
  ","
  ";"
] @punctuation.delimiter

[
  "{"
  "}"
  "["
  "]"
  "<"
  ">"
] @punctuation.bracket

(subgraph
  id: (id
    (identifier) @namespace)
)

(attribute
  name: (id
    (identifier) @attribute)
  value: (id
    (identifier) @constant)
)

(comment) @comment

(preproc) @keyword.directive

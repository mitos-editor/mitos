(function_declaration
  body: (_) @function.inside) @function.around

(struct_declaration) @class.around

[
  (struct_member)
  (parameter)
  (variable_declaration)
] @parameter.around

[(line_comment) (block_comment)] @comment.inside

[(line_comment) (block_comment)]+ @comment.around

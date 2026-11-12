(declaration
  (block
    [
      (declaration)
      (import)
      (variable)
      (spread_variable)
      (method_declaration)
      (comment)
      (block_comment)
    ]* @class.inside)) @class.around

(declaration) @entry.around @entry.movement
(import) @entry.around @entry.movement
(method_declaration) @function.around @function.movement
(argument_name) @parameter.inside @parameter.around @parameter.movement

[
  (comment)
  (block_comment)
] @comment.inside

[
  (comment)+
  (block_comment)+
] @comment.around

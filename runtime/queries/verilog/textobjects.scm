
(function_declaration
  (function_body_declaration
    [(block_item_declaration) (tf_item_declaration) (function_statement_or_null)]+ @function.inside)) @function.around

(comment) @comment.inside

(comment)+ @comment.around

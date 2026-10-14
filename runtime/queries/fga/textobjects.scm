(type_declaration) @class.around

(type_declaration
  (relations) @class.inside)

(condition_declaration
  body: (_) @function.inside) @function.around

(definition
  value: (_) @entry.inside) @entry.around

(condition_declaration
  ((param) @parameter.inside . ","? @parameter.around) @parameter.around)

(direct_relationship
  ((direct_relationship_item) @parameter.inside . ","? @parameter.around) @parameter.around)

(comment) @comment.inside

(comment)+ @comment.around

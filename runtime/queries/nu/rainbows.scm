[
  (block)
  (ctrl_match)
  (expr_interpolated)
  (expr_parenthesized)
  (parameter_bracks)
  (parameter_parens)
  (parameter_pipes)
  (val_binary)
  (val_closure)
  (val_list)
  (val_record)
  (val_table)
] @rainbow.scope

[
  "(" ")"
  "[" "]"
  "{" "}"
  "...(" "...[" "...{"
] @rainbow.bracket

(parameter_pipes "|" @rainbow.bracket)

[
  (collection_type ["<" ">"] @rainbow.bracket)
  (composite_type ["<" ">"] @rainbow.bracket)
  (list_type ["<" ">"] @rainbow.bracket)
] @rainbow.scope

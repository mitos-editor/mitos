(function_declaration
  body: (block)? @function.inside) @function.around

(func_literal
  body: (block)? @function.inside) @function.around

(method_declaration
  body: (block)? @function.inside) @function.around

; Struct and interface declarations, including constraints and embedded types.
(type_declaration
  (type_spec type: (struct_type (field_declaration_list) @class.inside))) @class.around

(type_declaration
  (type_spec type: (interface_type))) @class.around

; Capture the first and last members explicitly so mixed method/type lists
; form one inside selection. Empty interfaces still have an around selection.
(interface_type . (_) @class.inside .)
(interface_type . (_) @class.inside (_)* (_) @class.inside .)

(type_parameter_list
  ((_) @parameter.inside . ","? @parameter.around) @parameter.around)

(parameter_list
  ((_) @parameter.inside . ","? @parameter.around) @parameter.around)

(argument_list
  ((_) @parameter.inside . ","? @parameter.around) @parameter.around)

(comment) @comment.inside

(comment)+ @comment.around

; Go test/benchmark/example/fuzz functions (all live in _test.go).
((function_declaration
   name: (identifier) @_name
   body: (block)? @test.inside) @test.around
 (#match? @_name "^(Test|Benchmark|Example|Fuzz)"))

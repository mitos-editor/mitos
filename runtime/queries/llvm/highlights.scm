[
  (local_var)
  (global_var)
] @variable

(type) @type
(type_keyword) @type.builtin
(global_type (local_var) @type)

(type [
    (local_var)
    (global_var)
  ] @type)

(function_header
  (argument_list
    (argument (value (var (local_var) @variable.parameter)))))

(_ inst_name: _ @keyword.operator)

[
  "catch"
  "filter"
] @keyword.operator

[
  "to"
  "nneg"
  "nuw"
  "nsw"
  "exact"
  "disjoint"
  "unwind"
  "from"
  "cleanup"
  "swifterror"
  "volatile"
  "inbounds"
  "inrange"
  (icmp_cond)
  (fcmp_cond)
  (fast_math)
] @keyword.control

(_ callee: (value (var [(local_var) (global_var)] @function)))
(function_header name: _ @function)

[
  "declare"
  "define"
  (calling_conv)
] @keyword.function

[
  "target"
  "triple"
  "datalayout"
  "source_filename"
  "addrspace"
  "blockaddress"
  "align"
  "syncscope"
  "within"
  "uselistorder"
  "uselistorder_bb"
  "module"
  "asm"
  "sideeffect"
  "alignstack"
  "inteldialect"
  "unwind"
  "type"
  "global"
  "constant"
  "externally_initialized"
  "alias"
  "ifunc"
  "section"
  "comdat"
  "thread_local"
  "localdynamic"
  "initialexec"
  "localexec"
  "any"
  "exactmatch"
  "largest"
  "nodeduplicate"
  "samesize"
  "distinct"
  "attributes"
  "vscale"
  "no_cfi"
  (linkage_aux)
  (dso_local)
  (visibility)
  (dll_storage_class)
  (unnamed_addr)
  (attribute_name)
] @keyword


(function_header [
    (linkage)
    (calling_conv)
    (unnamed_addr)
  ] @keyword.function)

[
  (string)
  (cstring)
] @string

(number) @constant.numeric.integer
(comment) @comment
(label) @label
(_ inst_name: "ret" @keyword.control.return)
(float) @constant.numeric.float


[
  (struct_value)
  (array_value)
  (vector_value)
] @constructor

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
  "<"
  ">"
  "<{"
  "}>"
] @punctuation.bracket

[
  ","
  ":"
] @punctuation.delimiter

[
  "="
  "|"
  "x"
  "..."
] @operator

[
  "true"
  "false"
] @constant.builtin.boolean

[
  "undef"
  "poison"
  "null"
  "none"
  "zeroinitializer"
] @constant.builtin

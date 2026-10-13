; Variables

(identifier) @variable

(attribute attribute: (identifier) @variable.other.member)

((identifier) @constant
  (#match? @constant "^_*[A-Z][A-Z\\d_]*$"))

((identifier) @type
  (#match? @type "^[A-Z]"))

; Literals
(none) @constant.builtin
[
  (true)
  (false)
] @constant.builtin.boolean

(integer) @constant.numeric.integer
(float) @constant.numeric.float
(comment) @comment
(string) @string
(escape_sequence) @constant.character.escape

; Docstrings

(module . (string) @comment.block.documentation)
(function_definition body: (block . (string) @comment.block.documentation))
(class_definition body: (block . (string) @comment.block.documentation))
(trait_definition body: (block . (string) @comment.block.documentation))
(extension_definition body: (block . (string) @comment.block.documentation))

; Imports

(dotted_name
  (identifier)* @namespace)

(aliased_import
  (identifier) @namespace)

; Builtin functions

(call
  function: (attribute attribute: (identifier) @function.method))

(call
  function: (identifier) @function)


((call
  function: (identifier) @function.builtin)
  (#match?
    @function.builtin
    "^(abs|all|always_inline|any|ascii|bin|bool|breakpoint|bytearray|bytes|callable|chr|classmethod|compile|complex|constrained|delattr|dict|dir|divmod|enumerate|eval|exec|filter|float|format|frozenset|getattr|globals|hasattr|hash|help|hex|id|input|int|isinstance|issubclass|iter|len|list|locals|map|max|memoryview|min|next|object|oct|open|ord|pow|print|property|range|repr|reversed|round|set|setattr|slice|sorted|staticmethod|str|sum|super|tuple|type|unroll|vars|zip|__mlir_attr|__mlir_op|__mlir_type|__import__)$"))

; Function calls

[
  "def"
  "lambda"
] @keyword.function

(call
  function: (attribute attribute: (identifier) @constructor)
  (#match? @constructor "^[A-Z]"))

(call
  function: (identifier) @constructor
  (#match? @constructor "^[A-Z]"))

; Function definitions

(function_definition
  name: (identifier) @function)


(function_definition
  name: (identifier) @constructor
  (#match? @constructor "^(__new__|__init__|__moveinit__|__copyinit__)$"))

; Decorators

(decorator (identifier) @function)
(decorator (attribute attribute: (identifier) @function))
(decorator (call
  function: (attribute attribute: (identifier) @function)))

; Parameters

((identifier) @variable.builtin
  (#match? @variable.builtin "^(self|cls)$"))

(parameters (identifier) @variable.parameter)
(parameters (typed_parameter (identifier) @variable.parameter))
(parameters (default_parameter name: (identifier) @variable.parameter))
(parameters (typed_default_parameter name: (identifier) @variable.parameter))

(parameters
  (list_splat_pattern ; *args
    (identifier) @variable.parameter))

(parameters
  (dictionary_splat_pattern ; **kwargs
    (identifier) @variable.parameter))


; Types

((identifier) @type.builtin
  (#match?
    @type.builtin
    "^(bool|bytes|dict|float|frozenset|int|list|set|str|tuple)$"))

; In type hints make everything types to catch non-conforming identifiers
; (e.g., datetime.datetime) and None
(type [(identifier) (none)] @type)
; Handle [] . and | nesting 4 levels deep
(type
  (_ [(identifier) (none)]? @type
    (_ [(identifier) (none)]? @type
      (_ [(identifier) (none)]? @type
        (_ [(identifier) (none)]? @type)))))

(class_definition name: (identifier) @type)
(class_definition superclasses: (argument_list (identifier) @type))

["," "." ":" ";" (ellipsis)] @punctuation.delimiter
(interpolation
  "{" @punctuation.special
  "}" @punctuation.special) @embedded
["(" ")" "[" "]" "{" "}"] @punctuation.bracket

[
  "-"
  "-="
  "!="
  "*"
  "**"
  "**="
  "*="
  "/"
  "//"
  "//="
  "/="
  "&"
  "&="
  "%"
  "%="
  "^"
  "^="
  "+"
  "->"
  "+="
  "<"
  "<<"
  "<<="
  "<="
  "="
  ":="
  "=="
  ">"
  ">="
  ">>"
  ">>="
  "|"
  "|="
  "~"
  "@="
] @operator

[
  "as"
  "assert"
  "await"
  "from"
  "pass"
  "with"
] @keyword.control

[
  "if"
  "elif"
  "else"
  "case"
] @keyword.control.conditional

[
  "while"
  "for"
  "break"
  "continue"
] @keyword.control.repeat

[
  "return"
  "yield"
] @keyword.control.return

(yield "from" @keyword.control.return)

[
  "raise"
  "raises"
  "try"
  "except"
  "finally"
] @keyword.control.exception

(raise_statement "from" @keyword.control.exception)
"import" @keyword.control.import

(for_statement "in" @keyword.control)
(for_in_clause "in" @keyword.control)

[
  "alias"
  "async"
  "class"
  "struct"
  "trait"
  "comptime"
  "where"
  "capturing"
  "deinit"
] @keyword

[
  "and"
  "or"
  "not in"
  "in"
  "not"
  "del"
  "is not"
  "is"
] @keyword.operator

"var" @keyword.storage

[
  "read"
  "mut"
  "ref"
  "var"
  "comptime"
] @keyword.storage.modifier

((identifier) @type.builtin
  (#match? @type.builtin
    "^(BaseException|Exception|ArithmeticError|BufferError|LookupError|AssertionError|AttributeError|EOFError|FloatingPointError|GeneratorExit|ImportError|ModuleNotFoundError|IndexError|KeyError|KeyboardInterrupt|MemoryError|NameError|NotImplementedError|OSError|OverflowError|RecursionError|ReferenceError|RuntimeError|StopIteration|StopAsyncIteration|SyntaxError|IndentationError|TabError|SystemError|SystemExit|TypeError|UnboundLocalError|UnicodeError|UnicodeEncodeError|UnicodeDecodeError|UnicodeTranslateError|ValueError|ZeroDivisionError|EnvironmentError|IOError|WindowsError|BlockingIOError|ChildProcessError|ConnectionError|BrokenPipeError|ConnectionAbortedError|ConnectionRefusedError|ConnectionResetError|FileExistsError|FileNotFoundError|InterruptedError|IsADirectoryError|NotADirectoryError|PermissionError|ProcessLookupError|TimeoutError|Warning|UserWarning|DeprecationWarning|PendingDeprecationWarning|SyntaxWarning|RuntimeWarning|FutureWarning|ImportWarning|UnicodeWarning|BytesWarning|ResourceWarning)$"))

(trait_definition name: (identifier) @type)
(extension_definition name: (identifier) @type)
(parameterized_alias_statement name: (identifier) @type)
(convention_parameter (identifier) @variable.parameter)
(constrained_parameter name: (identifier) @variable.parameter)
(variadic_type_parameter name: (identifier) @type.parameter)
(struct_literal_field name: (identifier) @variable.other.member)
(inferred_attribute attribute: (identifier) @variable.other.member)
(self_parameter "self" @variable.builtin)
(self_type) @type.builtin

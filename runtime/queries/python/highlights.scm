; -------
; Punctuation
; -------

["," "." ":" ";" (ellipsis)] @punctuation.delimiter
["(" ")" "[" "]" "{" "}"] @punctuation.bracket
(interpolation
  "{" @punctuation.special
  "}" @punctuation.special)

; -------
; Operators
; -------

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
  "<>"
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

; -------
; Variables
; -------

(identifier) @variable

; - Member
(attribute attribute: (identifier) @variable.other.member)

; - Parameter
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

(lambda_parameters
  (identifier) @variable.parameter)

(keyword_argument
  name: (identifier) @variable.parameter)

; - Builtin
((identifier) @variable.builtin
 (#any-of? @variable.builtin "self" "cls"))

; -------
; Keywords
; -------

[
  "async"
  "class"
  "exec"
  "global"
  "nonlocal"
  "print"
  "type"
] @keyword

; Operators
[
  "and"
  "or"
  "not in"
  "in" ; Has to be before loop keywords because "in" is overloaded
  "not"
  "del"
  "is not"
  "is"
] @keyword.operator

; Control
[
  "as"
  "assert"
  "await"
  "from"
  "pass"

  "with"
] @keyword.control

; Conditionals
[
  "if"
  "elif"
  "else"
  "match"
  "case"
] @keyword.control.conditional

; Exceptions
[
  "raise"
  "try"
  "except"
  "finally"
] @keyword.control.exception
(raise_statement "from" @keyword.control.exception)

; Functions
[
  "def"
  "lambda"
] @keyword.function

; Import
"import" @keyword.control.import

; Loops
[
  "while"
  "for"
  "break"
  "continue"
] @keyword.control.repeat

(for_statement "in" @keyword.control.repeat)
(for_in_clause "in" @keyword.control.repeat)

; Return
[
  "return"
  "yield"
] @keyword.control.return
(yield "from" @keyword.control.return)

; -------
; Imports
; -------
 
(dotted_name
  (identifier)* @namespace)

(aliased_import
  alias: (identifier) @namespace)

; - Builtins
(none) @constant.builtin ; Has to be before types

; -------
; Types
; -------
 
((identifier) @type 
 (#match? @type "^[A-Z]")) ; Has to be before constructor due to this being a more general match 

; In type hints make everything types to catch non-conforming identifiers
; (e.g., datetime.datetime) and None
(type [(identifier) (none)] @type)
; Handle [] . and | nesting 4 levels deep
(type
  (_ [(identifier) (none)]? @type
    (_ [(identifier) (none)]? @type
      (_ [(identifier) (none)]? @type
        (_ [(identifier) (none)]? @type)))))

; Classes
(class_definition name: (identifier) @type.class.definition)
(class_definition superclasses: (argument_list (identifier) @type))

; -------
; Functions
; -------

(function_definition
  name: (identifier) @function.definition)

(call
  function: (identifier) @function.call)

; Methods
(call
  function: (attribute attribute: (identifier) @function.method.call))

(class_definition body: (block
  (function_definition name: (identifier) @function.method.definition)))
(class_definition body: (block (decorated_definition
  definition: (function_definition name: (identifier) @function.method.definition))))

; Builtin functions
((call
  function: (identifier) @function.builtin.call)
 (#any-of?
   @function.builtin.call
   "abs" "all" "any" "ascii" "bin" "breakpoint" "bytearray" "callable" "chr"
   "classmethod" "compile" "complex" "delattr" "dir" "divmod" "enumerate"
   "eval" "exec" "filter" "format" "getattr" "globals" "hasattr" "hash" "help"
   "hex" "id" "input" "isinstance" "issubclass" "iter" "len" "locals" "map"
   "max" "memoryview" "min" "next" "object" "oct" "open" "ord" "pow" "print"
   "property" "range" "repr" "reversed" "round" "setattr" "slice" "sorted"
   "staticmethod" "sum" "super" "type" "vars" "zip" "__import__"))

; Constructors
(call
  function: (attribute attribute: (identifier) @constructor.call)
  (#any-of?
    @constructor.call
    "__new__" "__init__"))

((call
  function: (identifier) @constructor.call)
 (#any-of?
   @constructor.call
   "__new__" "__init__"))

(function_definition
  name: (identifier) @constructor.definition
 (#any-of? @constructor.definition "__new__" "__init__"))

(call
  function: (attribute attribute: (identifier) @constructor.call)
 (#match? @constructor.call "^[A-Z]"))
(call
  function: (identifier) @constructor.call
 (#match? @constructor.call "^[A-Z]"))

; Builtin types
((identifier) @type.builtin ; Has to be after functions due to broad matching
 (#any-of?
   @type.builtin
   "bool" "bytes" "dict" "float" "frozenset" "int" "list" "set" "str" "tuple"))

; Builtin error types
((identifier) @type.builtin ; Has to be after constructors due to broad matching of constructor
  (#any-of? @type.builtin
    "BaseException" "Exception" "ArithmeticError" "BufferError" "LookupError"
    "AssertionError" "AttributeError" "EOFError" "FloatingPointError" "GeneratorExit"
    "ImportError" "ModuleNotFoundError" "IndexError" "KeyError" "KeyboardInterrupt"
    "MemoryError" "NameError" "NotImplementedError" "OSError" "OverflowError"
    "RecursionError" "ReferenceError" "RuntimeError" "StopIteration" "StopAsyncIteration"
    "SyntaxError" "IndentationError" "TabError" "SystemError" "SystemExit" "TypeError"
    "UnboundLocalError" "UnicodeError" "UnicodeEncodeError" "UnicodeDecodeError"
    "UnicodeTranslateError" "ValueError" "ZeroDivisionError" "EnvironmentError"
    "IOError" "WindowsError" "BlockingIOError" "ChildProcessError" "ConnectionError"
    "BrokenPipeError" "ConnectionAbortedError" "ConnectionRefusedError"
    "ConnectionResetError" "FileExistsError" "FileNotFoundError" "InterruptedError"
    "IsADirectoryError" "NotADirectoryError" "PermissionError" "ProcessLookupError"
    "TimeoutError" "Warning" "UserWarning" "DeprecationWarning" "PendingDeprecationWarning"
    "SyntaxWarning" "RuntimeWarning" "FutureWarning" "ImportWarning" "UnicodeWarning"
    "BytesWarning" "ResourceWarning"))

; -------
; Constants
; -------

((identifier) @constant
 (#match? @constant "^_*[A-Z][A-Z\\d_]*$"))

(escape_sequence) @constant.character.escape
(escape_interpolation) @constant.character.escape ; the `{{` and `}}` literals in f-strings

[
  (true)
  (false)
] @constant.builtin.boolean


; - Numbers
(integer) @constant.numeric.integer
(float) @constant.numeric.float

; -------
; Other literals
; -------
 
(comment) @comment
(string) @string

; -------
; Type aliases / type variables  (after the generic identifier+type rules so they win)
; -------

; PEP 695: `type Alias = ...`
(type_alias_statement
  . (type (identifier) @type.definition))

; `X = TypeVar("X")` / `X = NewType("X", int)`
((assignment
  left: (identifier) @type.definition
  right: (call
    function: (identifier) @_func))
 (#any-of? @_func "TypeVar" "NewType" "ParamSpec" "TypeVarTuple"))

; `X: TypeAlias = ...`
((assignment
  left: (identifier) @type.definition
  type: (type (identifier) @_ann))
 (#eq? @_ann "TypeAlias"))

; Decorator names are attributes, even when the decorator is a call expression.
(decorator (identifier) @attribute)
(decorator (attribute attribute: (identifier) @attribute))
(decorator (call function: (identifier) @attribute))
(decorator (call function: (attribute attribute: (identifier) @attribute)))

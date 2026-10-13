; Highlight queries for tree-sitter-lean.
;
; Capture names follow the nvim-treesitter capture convention, which maps
; back to Lean's SemanticTokenType vocabulary (see
;   lean4/src/Lean/Data/Lsp/LanguageFeatures.lean
; for the authoritative list). When the Lean LSP server is attached, its
; semantic tokens take precedence over these queries at runtime; the
; queries provide the floor for offline/cold-start highlighting.

; ---------- comments ------------------------------------------------------

(line_comment)        @comment
(block_comment)       @comment
(doc_comment)         @comment.block.documentation
(module_doc_comment)  @comment.block.documentation

; ---------- literals ------------------------------------------------------

(num_lit)             @constant.numeric.integer
(scientific_lit)      @constant.numeric.float
(char_lit)            @constant.character
(str_lit)             @string
(raw_string)          @string.special
(interpolated_str)    @string
(escape_sequence)     @constant.character.escape
(name_lit)            @string.special.symbol

(true_const)          @constant.builtin.boolean
(false_const)         @constant.builtin.boolean

((sorry) @keyword.control.exception)    ; Lean tags `sorry`/`admit` as leanSorryLike

; ---------- type-level constants ------------------------------------------

(type_const)          @type.builtin
(sort_const)          @type.builtin
(prop_const)          @type.builtin

; ---------- identifiers ---------------------------------------------------

; Plain identifier — default to variable. More-specific queries below
; override for declaration names, types, etc.
(identifier)          @variable

; Holes & meta-variables.
(hole)                @variable.builtin
(synth_hole)          @variable.builtin
(named_hole (identifier) @variable.builtin)

; ---------- declaration names --------------------------------------------

(def       name: (identifier) @function)
(theorem   name: (identifier) @function)
(abbrev    name: (identifier) @function)
(opaque    name: (identifier) @function)
(constant  name: (identifier) @function)
(axiom     name: (identifier) @function)
(instance  name: (identifier) @function)

(structure name: (identifier) @type)
(inductive name: (identifier) @type)

(ctor       name: (identifier) @constructor)
(ctor_alt   name: (identifier) @constructor)

(field      name: (identifier) @variable.other.member)

; Namespace / section names.
(namespace name: (identifier) @namespace)
(section   name: (identifier) @namespace)
(end       name: (identifier) @namespace)

; Universe names.
(universe  name: (identifier) @type.parameter)

; Import paths.
(import    name: (identifier) @namespace)
(open      namespace: (identifier) @namespace)
(open      only: (identifier)      @function)
(open      scoped: (identifier)    @namespace)
(export    namespace: (identifier) @namespace)
(export    only: (identifier)      @function)

; Attribute names.
(attributes name: (identifier) @attribute)

; Projection / field access.
(proj field: (identifier) @variable.other.member)
(proj field: (num_lit)    @constant.numeric.integer)

; Named-argument labels.
(paren arg_name: (identifier) @variable.parameter)

; Function call: highlight the called name as @function when it's a
; bare identifier.
(app fn: (identifier) @function)

; ---------- keywords ------------------------------------------------------

; Module-system keywords. `module`/`prelude`/`public`/`meta`/`all` each
; have a same-named rule wrapping the literal, so we query the rules
; rather than the bare strings.
(moduleTk)  @keyword.control.import
(prelude)   @keyword.control.import
(public)    @keyword.control.import
(meta)      @keyword.control.import
(all)       @keyword.control.import
"import"    @keyword.control.import
(public_section) @keyword.control.import

[
  "namespace"
  "section"
  "end"
] @keyword

[
  "def"
  "theorem"
  "example"
  "abbrev"
  "instance"
  "axiom"
  "opaque"
  "constant"
  "structure"
  "inductive"
  "class"
] @keyword

[
  "variable"
  "universe"
  "universes"
  "open"
  "export"
  "extends"
  "deriving"
  "where"
] @keyword

[
  "noncomputable"
  "partial"
  "nonrec"
  "private"
  "protected"
  "unsafe"
  "mut"
  "scoped"
  "local"
] @keyword.storage.modifier

[
  "#check"
  "#eval"
  "#print"
  "#reduce"
] @keyword.directive

[
  "fun"
  "λ"
] @keyword.function

[
  "let"
  "have"
  "show"
  "from"
] @keyword

[
  "if"
  "then"
  "else"
] @keyword.control.conditional

[
  "match"
  "with"
] @keyword.control.conditional

[
  "by"
  "do"
] @keyword

[
  "forall"
  "∀"
  "exists"
  "∃"
] @keyword.operator

; ---------- operators -----------------------------------------------------

[
  "→" "->" "↔" "<->" "↦" "=>"
  "∨" "||" "∧" "&&" "¬" "!"
  "=" "≠" "!=" "<" "≤" "<=" ">" "≥" ">="
  "∈" "∉" "⊆" "⊂" "⊇" "⊃"
  "≡" "≢" "~" "≃" "≅"
  "::" "++"
  "+" "-" "*" "/" "%"
  "∪" "∩" "\\"
  "^" "∘"
  "|>" "<|"
  ">>" "<<" ">>>" "<<<"
  ">>=" "=<<"
  "<$>" "<*>" "<|>" "<&>"
  "&&&" "|||" "^^^"
  "←" "<-"
] @operator

; ---------- punctuation ---------------------------------------------------

[":=" ":"] @operator
["," ";" "|"] @punctuation.delimiter
["(" ")" "{" "}" "[" "]" "⟨" "⟩" "⦃" "⦄" "{{" "}}"] @punctuation.bracket
["@[" "]"] @attribute

; ---------- raw atoms ----------------------------------------------------

["`" "'" "\"" "s!\"" "m!\""] @string

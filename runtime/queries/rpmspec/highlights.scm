(builtin) @variable.builtin

[
  (tag)
  (dependency_tag)
] @type.definition

(qualifier) @attribute.builtin

[
  "if"
  "else"
  "unless"
  "with"
  "without"
] @keyword.operator

(boolean_dependency
  "(" @punctuation.bracket
  ")" @punctuation.bracket)

(elf_dependency
  soname: (soname) @namespace
  symbol_version: (elf_symbol_version) @variable.other.member
  arch: (elf_arch) @attribute)

(path_dependency) @string.special.path

(qualified_dependency
  name: (_) @function
  qualifier: (dependency_qualifier
    content: (_) @variable.parameter))

((dependency_qualifier
   content: (word) @attribute)
 (#match? @attribute "^(x86-64|x86-32|aarch64|arm|ppc-64|ppc-32|s390x)$"))

(dependency
  (word) @namespace)

(preamble_tag
  value: (file) @string.special.path)

(description
  "%description" @type.definition)
(package
  "%package" @type.definition)

(sourcelist
  "%sourcelist" @type.definition)
(sourcelist
  (file) @string.special.path)

(patchlist
  "%patchlist" @type.definition)
(patchlist
  (file) @string.special.path)

(prep_scriptlet
  (section_prep) @function.builtin)
(generate_buildrequires
  (section_generate_buildrequires) @function.builtin)
(conf_scriptlet
  (section_conf) @function.builtin)
(build_scriptlet
  (section_build) @function.builtin)
(install_scriptlet
  (section_install) @function.builtin)
(check_scriptlet
  (section_check) @function.builtin)
(clean_scriptlet
  (section_clean) @function.builtin)

[
  "%pre"
  "%post"
  "%preun"
  "%postun"
  "%pretrans"
  "%posttrans"
  "%preuntrans"
  "%postuntrans"
  "%verify"
] @function.builtin

(script_interpreter
  "-p" @variable.parameter)
(interpreter_program) @string.special.path

(scriptlet_augment_option) @variable.parameter

[
  "%triggerprein"
  "%triggerin"
  "%triggerun"
  "%triggerpostun"
] @function.builtin

[
  "%filetriggerin"
  "%filetriggerun"
  "%filetriggerpostun"
  "%transfiletriggerin"
  "%transfiletriggerun"
  "%transfiletriggerpostun"
] @function.builtin

[
  (setup_macro argument: (macro_option) @variable.parameter)
  (autosetup_macro argument: (macro_option) @variable.parameter)
  (autopatch_macro argument: (macro_option) @variable.parameter)
  (patch_macro argument: (macro_option) @variable.parameter)
]

[
  (autopatch_macro (macro_argument) @constant.numeric.integer)
  (patch_macro (macro_argument) @constant.numeric.integer)
]

(files
  "%files" @type.definition)

[
  "%artifact"
  "%attr"
  "%caps"
  "%config"
  "%defattr"
  "%dir"
  "%doc"
  "%docdir"
  "%exclude"
  "%ghost"
  "%license"
  "%missingok"
  "%readme"
] @keyword.type

(changelog
  "%changelog" @type.definition)

(macro_definition
  "%" @punctuation.special
  ["define" "global"] @constant.builtin
  name: (identifier) @keyword.macro)

(macro_undefinition
  "%" @punctuation.special
    (identifier) @keyword.macro)

(macro_simple_expansion
  "%" @punctuation.special
  (simple_macro) @function.macro)

(macro_simple_expansion
  "%" @punctuation.special
  (negated_macro) @function.macro)

(macro_simple_expansion
  "%" @punctuation.special
  (special_macro) @function.macro)

(macro_parametric_expansion
  name: (identifier) @function.macro)

(macro_parametric_expansion
  option: (macro_option) @variable.parameter)

(macro_parametric_expansion
  argument: (word) @variable.parameter)

(macro_parametric_expansion
  argument: (integer) @constant.numeric.integer)

(macro_parametric_expansion
  argument: (quoted_string) @string)

(macro_option) @variable.parameter

(macro_expansion
  "%{" @punctuation.special
  "}" @punctuation.special) @none

(macro_expansion
    argument: (_) @variable.parameter)

(macro_expansion
  (identifier) @function.macro)

(macro_expansion
  (identifier)
  argument: [
    (word) @variable.parameter
    (concatenation
      (word) @variable.parameter)
  ])

(conditional_expansion
  condition: (identifier) @function.macro)

(special_variable_name) @constant

[
  "%if"
  "%ifarch"
  "%ifnarch"
  "%ifos"
  "%ifnos"
  "%elif"
  "%elifarch"
  "%elifos"
  "%else"
  "%endif"
] @keyword.control.conditional

[
  "defined"
  "undefined"
] @keyword.operator

(integer) @constant.numeric.integer
(float) @constant.numeric.float
(version) @constant.numeric.float

(quoted_string) @string

(url) @string.special.url

(comment) @comment

[
  "!="
  "<"
  "<="
  "="
  "=="
  ">"
  ">="
  "and"
  "&&"
  "or"
  "||"
] @operator

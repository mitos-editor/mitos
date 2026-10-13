; Copyright © 2024 Apple Inc. and the Pkl project authors. All rights reserved.
;
; Licensed under the Apache License, Version 2.0 (the "License");
; you may not use this file except in compliance with the License.
; You may obtain a copy of the License at
;
;     https://www.apache.org/licenses/LICENSE-2.0
;
; Unless required by applicable law or agreed to in writing, software
; distributed under the License is distributed on an "AS IS" BASIS,
; WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
; See the License for the specific language governing permissions and
; limitations under the License.

; this definition is imprecise in that 
; * any qualified or unqualified call to a method named "Regex" is considered a regex
; * string delimiters are considered part of the regex

; Operators

[
  "??"
  "@"
  "="
  "<"
  ">"
  "!"
  "=="
  "!="
  "<="
  ">="
  "&&"
  "||"
  "+"
  "-"
  "**"
  "*"
  "/"
  "~/"
  "%"
  "|>"
] @operator

[
  "?"
  "|"
  "->"
] @operator

[
  ","
  ":"
  "."
  "?."
] @punctuation.delimiter

[
  "("
  ")"
  "]"
  "{"
  "}"
  ; "[" @punctuation.bracket TODO: FIGURE OUT HOW TO REFER TO CUSTOM TOKENS
] @punctuation.bracket

; Keywords

[
  "abstract"
  "amends"
  "as"
  "class"
  "extends"
  "external"
  "function"
  "hidden"
  "import"
  "import*"
  "in"
  "let"
  "local"
  "module"
  "new"
  "open"
  "out"
  "typealias"
  "when"
] @keyword

[
  "if"
  "is"
  "else"
] @keyword.control.conditional

[
  "for"
] @keyword.control.repeat

(importExpr "import" @keyword.control.import)
(importExpr "import*" @keyword.control.import)

"read" @function.builtin
"read?" @function.builtin
"read*" @function.builtin
"throw" @function.builtin
"trace" @function.builtin

(moduleExpr "module" @type.builtin)
(nothingType) @type.builtin
(unknownType) @type.builtin

(outerExpr) @variable.builtin
"super" @variable.builtin
(thisExpr) @variable.builtin

[
  (falseLiteralExpr)
  (nullLiteralExpr)
  (trueLiteralExpr)
] @constant.builtin

; Literals

(stringConstant) @string
(slStringLiteralExpr) @string
(mlStringLiteralExpr) @string

(escapeSequence) @constant.character.escape

(intLiteralExpr) @constant.numeric.integer
(floatLiteralExpr) @constant.numeric.float

(stringInterpolation
  "\\(" @punctuation.special
  ")" @punctuation.special) @none

(stringInterpolation
 "\\#(" @punctuation.special
 ")" @punctuation.special) @none

(stringInterpolation
  "\\##(" @punctuation.special
  ")" @punctuation.special) @none

(lineComment) @comment
(blockComment) @comment
(docComment) @comment

; Identifiers

(identifier) @variable
((identifier) @type
 (#match? @type "^[A-Z]"))

(classProperty (identifier) @variable.other.member)
(objectProperty (identifier) @variable.other.member)
; Property access `o.field` (unqualifiedAccessExpr below handles `o.method()`).
(qualifiedAccessExpr (identifier) @variable.other.member)

(parameterList (typedIdentifier (identifier) @variable.parameter))
(objectBodyParameters (typedIdentifier (identifier) @variable.parameter))

; Method definitions

(classMethod (methodHeader (identifier) @function.method))
(objectMethod (methodHeader (identifier) @function.method))

; Method calls

(unqualifiedAccessExpr
  (identifier) @function
  (argumentList))
(qualifiedAccessExpr
  (identifier) @function.method
  (argumentList))

; Types

(clazz (identifier) @type)
(typeAlias (identifier) @type)


(typeArgumentList
  "<" @punctuation.bracket
  ">" @punctuation.bracket)

(shebangComment) @keyword.directive

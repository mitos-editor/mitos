; highlights.scm

(comment) @comment

[
    "ELEMENT"
    "ATTLIST"
] @keyword

[
    "#REQUIRED"
    "#IMPLIED"
    "#FIXED"
    "#PCDATA"
] @keyword.directive

[
    "EMPTY"
    "ANY"
    "SYSTEM"
    "PUBLIC"
] @constant

(element_name) @tag


(attribute_name) @attribute

(system_literal) @string
(pubid_literal) @string
(attribute_value) @string

[
    ">"
    "</"
    "<?"
    "?>"
    "<!"
] @punctuation.bracket

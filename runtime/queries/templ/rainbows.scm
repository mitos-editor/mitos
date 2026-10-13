; inherits: go

[
  (expression)
  (component_switch_statement)
  (component_type_switch_statement)
  (component_render)
  (component_children_expression)
  (doctype)
  (spread_attributes)
  (conditional_attribute_block)
  (css_declaration)
  (dynamic_class_attribute_value)
  (component_block)
  (script_block)
  (rawgo_block)
  (literal_value)
] @rainbow.scope

[
  "{"
  "}"
  "{!"
  "}"
  "{{"
  "}}"
] @rainbow.bracket

([
    (element)
    (style_element)
    (script_element)
  ] @rainbow.scope
  (#set! rainbow.include-children))

; Markup delimiters are scoped to tags so Go comparisons stay operators.
(tag_start ["<" ">"] @rainbow.bracket)
(tag_end ["</" ">"] @rainbow.bracket)
(self_closing_tag ["<" "/>"] @rainbow.bracket)
(style_tag_start ["<" ">"] @rainbow.bracket)
(style_tag_end ["</" ">"] @rainbow.bracket)
(self_closing_style_tag ["<" "/>"] @rainbow.bracket)
(script_tag_start ["<" ">"] @rainbow.bracket)
(script_tag_end ["</" ">"] @rainbow.bracket)
(self_closing_script_tag ["<" "/>"] @rainbow.bracket)
(doctype ["<!" ">"] @rainbow.bracket)
(void_element ["<" ">"] @rainbow.bracket) @rainbow.scope

; inherits: html

((frontmatter
  (frontmatter_js_block) @injection.content)
 (#set! injection.language "typescript"))

((attribute_interpolation
  (attribute_js_expr) @injection.content)
 (#set! injection.language "typescript"))

((html_interpolation
  (permissible_text) @injection.content)
 (#set! injection.language "tsx"))

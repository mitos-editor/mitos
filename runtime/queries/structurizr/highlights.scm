; ==============================================================================
; Primitive literals and built-in value families
; ==============================================================================

(comment) @comment

[
  (string)
  (text_block_string)
] @string

[
  (hex_color)
  (named_color)
  (shape_value)
] @string.special

[
  (border_value)
  (icon_position_value)
  (line_style_value)
  (routing_value)
  (wildcard)
  (layout_direction)
  (filter_mode)
] @constant.builtin

(number) @constant.numeric

(order) @constant.numeric

(boolean_value) @constant.builtin.boolean

(decision_importer_type) @type.builtin

(java_fully_qualified_name) @type

(this_keyword) @variable.special

(relationship_operator) @operator

(default_statement) @keyword

; ==============================================================================
; Directive and statement keywords
; ==============================================================================

[
  "!include"
  "!const"
  "!constant"
  "!var"
  "!identifiers"
  "!impliedRelationships"
  "!docs"
  "!adrs"
  "!elements"
  "!relationships"
  "!element"
] @keyword.directive

[
  "workspace"
  "extends"
  "model"
  "views"
  "configuration"
  "archetypes"
  "group"
  "enterprise"
  "person"
  "softwareSystem"
  "softwaresystem"
  "container"
  "component"
  "deploymentEnvironment"
  "deploymentGroup"
  "deploymentNode"
  "infrastructureNode"
  "containerInstance"
  "softwareSystemInstance"
  "instanceOf"
  "description"
  "technology"
  "tag"
  "tags"
  "metadata"
  "url"
  "name"
  "value"
  "title"
  "include"
  "exclude"
  "autoLayout"
  "autolayout"
  "animation"
  "systemLandscape"
  "systemlandscape"
  "systemContext"
  "systemcontext"
  "filtered"
  "dynamic"
  "deployment"
  "custom"
  "styles"
  "light"
  "dark"
  "element"
  "relationship"
  "properties"
  "theme"
  "themes"
  "branding"
  "logo"
  "font"
  "perspectives"
  "perspective"
  "healthCheck"
  "plantuml"
  "mermaid"
  "kroki"
  "image"
  "terminology"
  "scope"
  "visibility"
  "users"
] @keyword

; ==============================================================================
; Path-like, URL-like, and selector-like values
; ==============================================================================

; URL-bearing statements accept bare values as well as quoted strings, so capture the
; field directly to keep health checks, fonts, and ordinary URLs visually coherent.
(url_statement
  value: [
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special.url)

(health_check_statement
  url: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special.url)

(font_statement
  url: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special.url)

; Directive and source paths often parse as bare values or identifiers rather than
; strings. Highlight them explicitly so the richer directive nodes are fully represented.
(include_directive
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(docs_directive
  path: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(adrs_directive
  path: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(theme_statement
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(themes_statement
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(logo_statement
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(plantuml_source
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(mermaid_source
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(kroki_source
  format: [
    (identifier)
    (bare_value)
    (string)
  ] @type.builtin)

(kroki_source
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(image_source
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(elements_directive
  expression: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(relationships_directive
  expression: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(element_directive
  target: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

; ==============================================================================
; Semantic value slots
; ==============================================================================

(const_directive
  name: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @constant)

(constant_directive
  name: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @constant)

(var_directive
  name: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @variable)

(const_directive
  value: [
    (identifier)
    (bare_value)
    (text_block_string)
  ] @string)

(constant_directive
  value: [
    (identifier)
    (bare_value)
    (text_block_string)
  ] @string)

(var_directive
  value: [
    (identifier)
    (bare_value)
    (text_block_string)
  ] @string)

(identifiers_directive
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @constant.builtin)

(implied_relationships_directive
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @constant.builtin.boolean)

(name_statement
  value: [
    (identifier)
    (bare_value)
    (text_block_string)
  ] @string)

(description_statement
  value: [
    (identifier)
    (bare_value)
    (text_block_string)
  ] @string)

(technology_statement
  value: [
    (identifier)
    (bare_value)
    (text_block_string)
  ] @string)

(metadata_statement
  value: [
    (identifier)
    (bare_value)
    (text_block_string)
  ] @string)

(value_statement
  value: [
    (identifier)
    (bare_value)
    (text_block_string)
  ] @string)

(title_statement
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string.special)

(custom_view
  title: [
    (identifier)
    (string)
  ] @string.special)

(scope_statement
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @constant.builtin)

(visibility_statement
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @constant.builtin)

(font_statement
  name: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string)

(health_check_statement
  name: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @label)

(perspective_definition
  name: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @label)

(perspective_entry
  name: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @label)

(perspective_entry
  description: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string)

(perspective_entry
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string)

(terminology_entry
  kind: (identifier) @type.builtin)

(terminology_entry
  value: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string)

(user_entry
  username: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string)

(user_entry
  role: [
    (identifier)
    (bare_value)
    (string)
    (text_block_string)
  ] @string)

; ==============================================================================
; Brackets and operators
; ==============================================================================

[
  "{"
  "}"
] @punctuation.bracket

":" @punctuation.delimiter

"=" @operator

"->" @operator

; ==============================================================================
; Declarations and references
; ==============================================================================

(group
  identifier: (identifier) @variable)

(person
  identifier: (identifier) @variable)

(software_system
  identifier: (identifier) @type)

(container
  identifier: (identifier) @type)

(component
  identifier: (identifier) @type)

(custom_element
  identifier: (identifier) @type)

(archetype_definition
  identifier: (identifier) @type)

(archetype_instance
  kind: (identifier) @type)

(deployment_environment
  identifier: (identifier) @type)

(deployment_group
  identifier: (identifier) @type)

(deployment_node
  identifier: (identifier) @type)

(infrastructure_node
  identifier: (identifier) @type)

; Deployment instances carry their assignment, target, and optional deployment group
; as named fields directly on the instance node, so highlight each role explicitly.
(container_instance
  identifier: (identifier) @type)

(container_instance
  target: (identifier) @type)

(container_instance
  deployment_group: (identifier) @type)

(software_system_instance
  identifier: (identifier) @type)

(software_system_instance
  target: (identifier) @type)

(software_system_instance
  deployment_group: (identifier) @type)

(instance_of
  identifier: (identifier) @type)

(instance_of
  target: (identifier) @type)

(instance_of
  deployment_group: (identifier) @type)

(relationship
  identifier: (identifier) @variable)

(relationship
  source: (identifier) @variable
  destination: (identifier) @variable)

(dynamic_relationship
  source: (identifier) @variable
  destination: (identifier) @variable)

(dynamic_relationship_reference
  relationship: (identifier) @variable)

(system_context_view
  scope: (identifier) @type)

(container_view
  scope: (identifier) @type)

(component_view
  scope: (identifier) @type)

(dynamic_view
  scope: (identifier) @type)

(deployment_view
  scope: (identifier) @type)

(include_statement
  value: (identifier) @variable)

(include_statement
  value: [
    (bare_value)
    (string)
  ] @string.special)

(exclude_statement
  value: (identifier) @variable)

(exclude_statement
  value: [
    (bare_value)
    (string)
  ] @string.special)

(animation_block
  value: (identifier) @variable)

(animation_block
  value: [
    (bare_value)
    (string)
  ] @string.special)

; Style rule tags and filtered-view tags represent the same DSL tag surface and
; should share the same semantic styling rather than falling back to plain strings.
(tag_statement
  value: [
    (identifier)
    (string)
  ] @tag)

(tags_statement
  value: [
    (identifier)
    (string)
  ] @tag)

(filtered_view
  tags: [
    (identifier)
    (string)
  ] @tag)

(element_style
  tag: [
    (identifier)
    (string)
  ] @tag)

(relationship_style
  tag: [
    (identifier)
    (string)
  ] @tag)

(style_setting
  name: (identifier) @variable.other.member)

(property_entry
  name: [
    (identifier)
    (bare_value)
    (string)
  ] @variable.other.member)

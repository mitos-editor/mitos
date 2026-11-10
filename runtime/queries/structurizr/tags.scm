; Workspace sections
(workspace name: (_) @name) @definition.module
(workspace !name "workspace" @name) @definition.module

(model "model" @name) @definition.section
(views "views" @name) @definition.section
(configuration "configuration" @name) @definition.section
(archetypes "archetypes" @name) @definition.section
(styles "styles" @name) @definition.section
(branding "branding" @name) @definition.section
(terminology "terminology" @name) @definition.section

; Model elements
(person name: (_) @name) @definition.class
(group name: (_) @name) @definition.class
(enterprise name: (_) @name) @definition.class
(software_system name: (_) @name) @definition.class
(container name: (_) @name) @definition.class
(component name: (_) @name) @definition.class
(custom_element name: (_) @name) @definition.class
(deployment_environment name: (_) @name) @definition.class
(deployment_group name: (_) @name) @definition.class
(deployment_node name: (_) @name) @definition.class
(infrastructure_node name: (_) @name) @definition.class

(archetype_definition identifier: (identifier) @name) @definition.type

; Views
(system_landscape_view "systemLandscape" @name) @definition.section
(system_context_view scope: (identifier) @name) @definition.section
(container_view scope: (identifier) @name) @definition.section
(component_view scope: (identifier) @name) @definition.section
(filtered_view base_key: (_) @name) @definition.section
(dynamic_view scope: (_) @name) @definition.section
(deployment_view environment: (_) @name) @definition.section
(custom_view "custom" @name) @definition.section
(image_view scope: (_) @name) @definition.section

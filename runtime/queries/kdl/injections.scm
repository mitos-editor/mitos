; Intermediate wrappers use fields to avoid expensive recursive grammar analysis.
; Node names and fields retain the Niri section and argument constraints.
; match/exclude regex with regular string
(node
  name: (identifier) @_section_name
  (#any-of? @_section_name "window-rule" "layer-rule")
  children: (_
    (node
      name: (identifier) @_node_name
      (#any-of? @_node_name "match" "exclude")
      (_
        property: (_
          key: (identifier) @_prop_name
          (#any-of? @_prop_name "app-id" "title" "namespace")
          value: (_
            value: (_
              (string_fragment) @injection.content
              (#set! injection.language "regex")
            )
          )
        )
      )
    )
  )
)

(node
  name: (identifier) @_section
  (#eq? @_section "binds")
  children: (_
    (node
      name: (identifier)
      children: (_
        (node
          name: (identifier) @_action_name
          (#eq? @_action_name "spawn")
          (_
            argument: (_
              value: (_
                (string_fragment) @_executable
                (#eq? @_executable "fish")
              )
            )
          )
          (_
            argument: (_
              value: (_
                (string_fragment) @_flag
                (#eq? @_flag "-c")
              )
            )
          )
          (_
            argument: (_
              value: (_
                (string_fragment) @injection.content
                (#set! injection.language "fish")
              )
            )
          )
        )
      )
    )
  )
)

([
  (single_line_comment)
  (multi_line_comment)
] @injection.content
  (#set! injection.language "comment"))

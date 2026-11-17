; inherits: yaml

; https://docs.docker.com/reference/compose-file/build/#dockerfile_inline
(block_mapping_pair
  key: (flow_node) @_key
  value: (block_node
    (block_scalar) @injection.content)
  (#any-of? @_key "dockerfile_inline" "\"dockerfile_inline\"" "'dockerfile_inline'")
  (#set! injection.language "dockerfile")
)

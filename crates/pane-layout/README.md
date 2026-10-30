# pane-layout

Split topology, stable pane keys, weighted rectangle allocation with caller-supplied
minimum sizes, traversal, and directional navigation. Callers supply pane values
implementing `Pane` and retain ownership of document bindings, service lifetimes,
and rendering.

Mitos adapts the layout through `view::tree::Tree`. Structural editor changes stay
behind `Editor` so closing or splitting a pane also updates document selections
and services.

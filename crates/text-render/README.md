# `text-render`

Paints formatted text, highlight layers, whitespace markers, indent guides, and
anchored decorations into Ratatui buffers. Inputs are text, layout, resolved
styles, and explicit rendering settings. No editor, protocol, syntax parser, or
terminal backend is required.

The frontend prepares document settings and adapts parser highlights into a lazy
stream of character positions and styles. Feature-specific decorations, such as
code lenses, debugger values, and cursor tracking, remain with their frontend.

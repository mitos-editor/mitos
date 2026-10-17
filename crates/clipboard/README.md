# `clipboard`

Owns clipboard provider settings, the backend interface, native provider detection,
and OS or command-based access. It has no editor, configuration service, or
terminal output dependency. `view` supplies live settings and register semantics;
`term` supplies OSC 52 output and terminal-specific provider selection.

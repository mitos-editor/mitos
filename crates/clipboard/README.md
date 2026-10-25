# `clipboard`

Owns clipboard provider settings, the backend interface, native provider detection,
and OS or command-based access. It has no editor, configuration service, or
terminal output dependency. `view` supplies live settings and register semantics;
`term` supplies OSC 52 output and terminal-specific provider selection.

Native and terminal defaults share platform detection. A frontend that supports
OSC 52 uses `ClipboardProvider::default_for_terminal`; the ordinary default never
requires terminal output.

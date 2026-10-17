# `text-io`

Streaming decoding to ropes or strings, encoding detection, BOM handling, and
encoded writes to async streams. Owns encoding algorithms and their fixtures;
has no document, parser, protocol, or editor dependency. Document file sniffing,
metadata, history, conflict handling, and save coordination remain in `view`.

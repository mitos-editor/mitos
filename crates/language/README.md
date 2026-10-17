# Language

Language definitions, syntax analysis, query compilation, and structural editing.
Depends on text primitives in `core`, never on editor state or a frontend.

`config` defines language metadata; `loader` loads configuration layers; `syntax`
owns incremental trees and language lookup; `queries` compiles supplied queries.
Text-only algorithms remain in `core`.

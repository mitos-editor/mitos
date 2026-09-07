# `view`

`view` owns Mitos's backend-independent editor model: documents, views, split trees, editor state, input actions, registers, themes, annotations, and protocol-facing state updates.

Documents initialize syntax on background workers so opening a file does not delay the first frame. `document::syntax_initialization` owns snapshots and their `TaskController`; the terminal syntax handler uses the existing job queue to publish completed trees on the editor thread. Trees are only installed for the current text version, and language changes or document closure cancel pending work. Consumers must handle `Document::syntax()` returning `None` while initialization is pending, just as they do for files without a grammar.

Explicit commands that need a tree, such as opening a syntax symbol picker, can call `Editor::ensure_syntax` to finish initialization on demand. Explicit language changes also initialize syntax immediately so subsequent commands use the new language's indentation and syntax rules.

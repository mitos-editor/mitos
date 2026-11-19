# Code folding

Fold code to keep long functions and other blocks out of the way while you work.
Syntax and selection folds keep the first line visible and show a selectable
`…` in place of the remaining lines, including closing delimiters and any text
on their lines. For example, a folded Rust struct looks like
`pub struct Editor {…`, and a Python function looks like `def run():…`.
Folding changes the view, leaving the document text and undo history intact.
Each split has its own folds.

Use these view commands to fold code:

| Key | Command | Action |
| --- | --- | --- |
| `z c` | `fold_close` | Close the innermost open fold at each cursor |
| `z o` | `fold_open` | Open the outermost closed fold at each cursor |
| `z a` | `fold_toggle` | Toggle the fold at each cursor |
| `z f` | `fold_selection` | Fold the lines covered by each selection |
| `z C` | `fold_all` | Close all folds |
| `z O` | `unfold_all` | Open all folds, including nested folds |
| `z A` | `fold_toggle_all` | Open all folds if any are closed; otherwise close all folds |

Use a count to close or open several nested folds, for example `2zc`. Opening
an outer fold keeps any folds inside it closed. `Z` provides the same commands
from the sticky view menu. Use `z z` to center the view vertically.

Folding uses the first configured language server that provides folding ranges.
Server ranges can fold part of a line and keep the surrounding text visible.
Servers can also provide a collapsed label in place of `…`; clicking any part
of the label selects the fold's start. Wrapped labels remain scrollable, and
cursor motions cross their continuation rows as a unit. When no server provides
ranges, or its request fails, Mitos uses Tree-sitter. Check the folding column in
[Language support](./lang-support.md) for syntax-based support.

Use `fold_comments`, `fold_imports`, or `fold_regions` from the command palette,
or bind them to keys, to close all server folds of that kind. These commands
leave folds of other kinds unchanged. `fold_all` includes unrecognized server
kinds. No folds close automatically.

To fold text in any document, select at least two lines and press `z f`.
Folds can contain other folds, but cannot partly overlap. Opening a selection
fold keeps its boundaries available for `z c`, `z C`, `z a`, and `z A`.
Editing inside that region or changing the document's language clears its saved
boundaries.

`j` and `k` count visible document lines; wrapped-line motions count visual rows.
Relative line numbers count visible lines too. Scrolling and mouse clicks follow
the visible text. Searching, jumping, or otherwise moving a cursor into hidden
text opens the folds covering its destination. Editing hidden text opens the
affected folds in all splits. Other edits, including undo and redo, keep folds
with their text.
Changing a document's language clears its folds.

Selections still refer to the underlying document, so an operation on a
selection spanning a fold includes its hidden text. Moving horizontally or by
words into hidden text reveals it. Folding is not saved between editor sessions.

Themes can style collapsed labels with `ui.virtual.fold`.

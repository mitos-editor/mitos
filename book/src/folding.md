# Code folding

Fold code to keep long functions and other blocks out of the way while you work.
A closed fold shows a selectable `…` in place of its body. Syntax folds keep
closing braces, brackets, and parentheses visible with the rest of their line,
so a folded region can look like `pub struct Editor {…}`, `[…],`, or
`call!(…);`. You can select and edit the closing tokens normally.

Selection folds and syntax regions without paired closing tokens fold whole
lines. Folding changes the view, leaving the document text and undo history
intact. Each split has its own folds.

Press `z f` to open the folding menu:

| Key | Command | Action |
| --- | --- | --- |
| `z f c` | `fold_close` | Close the innermost open syntax fold at each cursor |
| `z f o` | `fold_open` | Open the outermost closed fold at each cursor |
| `z f a` | `fold_toggle` | Toggle the fold at each cursor |
| `z f s` | `fold_selection` | Fold the lines covered by each selection |
| `z f m` | `fold_all` | Close all syntax folds |
| `z f r` | `unfold_all` | Open all folds, including nested folds |

Use a count to close or open several nested folds, for example `2zfc`. Opening
an outer fold keeps any folds inside it closed. `Z f` provides the same commands
from the sticky view menu.

Syntax folding requires support for the document's language. Check the folding
column in [Language support](./lang-support.md). No language server is required.
To fold text in any document, select at least two lines and press `z f s`.
Folds can contain other folds, but cannot partly overlap.

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

Themes can style the ellipsis with `ui.virtual.fold`.

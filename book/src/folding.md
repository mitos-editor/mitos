# Code folding

Folding hides whole lines behind a selectable `…` at the end of the first line.
It changes the view, leaving the document text and undo history intact. Each
split has its own folds.

Press `z f` to open the folding menu:

| Key | Command | Action |
| --- | --- | --- |
| `z f c` | `fold_close` | Close the innermost open syntax fold at each cursor |
| `z f o` | `fold_open` | Open the outermost closed fold at each cursor |
| `z f a` | `fold_toggle` | Toggle the fold at each cursor |
| `z f s` | `fold_selection` | Fold the lines covered by each selection |
| `z f m` | `fold_all` | Close all syntax folds |
| `z f r` | `unfold_all` | Open all folds, including nested folds |

A count repeats `fold_close` or `fold_open`. Opening a parent keeps its closed children. `Z f` provides the same
commands from the sticky view menu.

Syntax folds use Tree-sitter and the language's `folds.scm` query. The
[language support table](./lang-support.md) lists languages with folding queries.
No language server is required. Selection folding works in any text document;
a selection must cover at least two lines. Crossing fold regions are ignored so
folds remain properly nested.

`j` and `k` count visible document lines; wrapped-line motions count visual rows.
Scrolling and mouse positioning follow the folded layout. Searching, jumping,
or otherwise moving a cursor into hidden text opens the folds covering its
destination. An edit that touches concealed text opens the affected folds in
all splits. Other edits, including undo and redo, move folds with their text.
Changing a document's language clears its folds.

Selections still refer to the underlying document, so an operation on a
selection spanning a fold includes its hidden text. Moving horizontally or by
words into hidden text reveals it. Folding is not saved between editor sessions.

Themes can style the ellipsis with `ui.virtual.fold`.

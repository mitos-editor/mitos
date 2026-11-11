# Edit code with textobjects

A textobject selects a meaningful piece of text: a word, quoted string,
argument, or function. Once it is selected, the same editing commands work on
any of them. This article starts with words and quotes, then uses functions,
arguments, and the surrounding code structure to navigate and make changes.

Start with [Basics](../basics.md) if you have not made your first edit yet.
Use the default keys below, and open a fresh scratch buffer with `:new` for the
first exercise. Type or paste the starting text, then press `Esc`.

## Change a word or the text inside quotes

Start with:

```text
status = "draft"
```

1. Move anywhere inside `status` and press `miw` to select the whole word.
2. Check that only `status` is selected. Press `c`, type `state`, then `Esc`.
3. Move inside `draft` and press `mi"` to select the text inside the quotes.
4. Press `c`, type `ready`, then `Esc`.

The result is:

```text
state = "ready"
```

You do not need to move to the start of a word or count its letters.
`mi` selects the inside of a [textobject](../textobjects.md); `ma` includes its
surroundings. For example, `ma"` selects the quotes as well as their contents.
Use `mi"` when the quotes should stay in place.

**Try it:** change `ready` to `in review`, keeping the quotes. Then use `u` to
undo that edit and `U` to redo it.

## Move between functions

Save this example as `greetings.rs` and open it in Mitos. The `.rs` extension
selects Rust syntax support. In an unnamed scratch buffer, use `:lang rust`
instead.

```rust
fn greet(name: &str) -> String {
    format!("Hello, {name}")
}

fn farewell(name: &str) -> String {
    format!("Goodbye, {name}")
}
```

1. Put the cursor inside the `greet` function.
2. Press `]f` to move to the next function, `farewell`.
3. Press `[f` to return to `greet`.
4. Press `maf` to select the whole function. Check that `farewell` is outside
   the selection.

Use these motions when you want the next function rather than a particular line
number. `[a` and `]a` move between parameters; `[t` and `]t` move between types.
See the [textobject reference](../textobjects.md) for other kinds of object.

Code objects need a syntax grammar and textobject queries for the language;
they do not need a language server. Check [Language support](../lang-support.md)
if these commands do not select anything. Words and quoted pairs also work in
plain text.

## Limit a change to one function

In the example above, both functions have a parameter named `name`. Change it
to `guest` only in `greet`:

1. Put the cursor inside `greet` and press `maf` to select the whole function,
   including its signature.
2. Press `s`, type `\bname\b`, then `Enter`. Only the two occurrences in the
   selected function should be highlighted.
3. Press `Alt-r`, type `guest`, then `Enter`.

The result is:

```rust
fn greet(guest: &str) -> String {
    format!("Hello, {guest}")
}

fn farewell(name: &str) -> String {
    format!("Goodbye, {name}")
}
```

`maf` selects the whole function; `mif` selects its body. The exact boundaries
depend on the language: in Rust, the body selection includes its braces. For
an edit that includes a parameter in the signature, use `maf`. For copying a
body, start with `mif`, inspect the selection, then press `y`.

The search is still a text match. It will also match `name` in a comment or
string inside the selection. Use `Space r` with a language server that supports
renaming when you want to change a code symbol and its references.

After working with several matches, press `,` to keep one selection and `;` to
collapse it to its cursor before navigating elsewhere.

## Select an argument as one piece

A word is not always the right unit. Open a fresh scratch buffer with `:new`,
set its language with `:lang rust`, and enter this statement. Press `Esc` when
you finish typing:

```rust
let label = greet(user.name());
```

Put the cursor on `name` and press `mia`. The argument `user.name()` should be
selected, including the method call but excluding `greet` and its outer
parentheses. Press `c`, type `guest.name()`, then `Esc`:

```rust
let label = greet(guest.name());
```

This works the same way as changing a word: select a useful unit, inspect it,
then act. Next, use [syntax-aware motions](../syntax-aware-motions.md) to move
between related pieces or select the expression around them.

## Move between siblings

Siblings are pieces of syntax within the same enclosing structure. For example,
the arguments in a call share an argument list. Sibling motions follow that
structure, including punctuation such as commas and brackets.

Open a fresh Rust scratch buffer with `:new`, then `:lang rust`, for each of the
remaining exercises. Enter the starting example and press `Esc`:

```rust
let total = add(first, second, third);
```

1. Put the cursor on `first` and press `miw`.
2. Press `Alt-n` once. In this Rust example, the comma is selected.
3. Press `Alt-n` again to select `second`.
4. Press `Alt-p` twice to return through the comma to `first`.
5. Press `Alt-n` twice to select `second` again. Press `c`, type `updated`,
   then `Esc`.

The result is:

```rust
let total = add(first, updated, third);
```

`Alt-n` and `Alt-p` work on expressions and statements as well as arguments.
If there is no sibling in that direction, they can move out to an enclosing
structure to find one. Inspect the highlight after each move; the selection
can leave the argument list. Use `]a` and `[a` when you specifically want to
navigate by argument or parameter textobjects.

## Expand to a parent and shrink back

A parent contains the selected piece. In the original `add(first, second,
third)` example, `second` belongs to an argument list, and that list belongs to
the call. Starting from the unedited example in a fresh Rust buffer:

1. Put the cursor on `second` and press `miw`.
2. Press `Alt-o` to select `(first, second, third)`, including the parentheses.
3. Press `Alt-o` again to select `add(first, second, third)`.
4. Press `Alt-i` to return to the argument list, then `Alt-i` again to return
   to `second`.

You can now change just `second` with `c`, copy it with `y`, or expand again
before acting on a larger expression.

This is useful when you know the code you want by its shape rather than its
length. For example, expand to the whole call before copying it, so the
selection includes the arguments and closing parenthesis.

`Alt-i` retraces expansions while their selection history is available. Moving
elsewhere, changing the selections another way, or editing clears that expansion
history. Without it, shrinking selects a child of the current structure;
it may not return to the piece you started from.

## Edit siblings or children together

Use `Alt-a` when you want separate selections for related pieces instead of
moving through them one at a time. Start with:

```rust
let values = [first, second, third];
```

1. Put the cursor on `first` and press `miw`.
2. Press `Alt-a`. Check that the three values are selected separately, leaving
   the commas and brackets outside the selections.
3. Press `ms"` to add double quotes around each selection.

The result is:

```rust
let values = ["first", "second", "third"];
```

You can reach the same selections from their parent. Press `u` to undo the
quotes, then `,` to keep one selection. With its cursor inside the array:

1. Press `ma[` to select the array including its brackets.
2. Press `Alt-I` to select its children separately. This is uppercase `I`:
   hold Alt and Shift while pressing `i`.
3. Check the three selected values, then press `ms"` to get the same result.

`Alt-a` selects siblings, including the starting piece; `Alt-I` selects the
children of the selected structure. In this array, both methods select the
values without punctuation. The pieces available depend on the language grammar.
See [Surround](../surround.md) for other ways to wrap selected text.

Press `,` to keep one selection when you finish. Before making an edit, you can
use `Alt-u` to undo an unwanted selection change.

## Choose a useful unit

| What you want | Start with |
| --- | --- |
| A particular kind of object, such as a word or argument | `miw`, `mia`, or another textobject |
| The next or previous function or parameter | `]f` / `[f` or `]a` / `[a` |
| The next or previous piece of syntax, including punctuation | `Alt-n` or `Alt-p` |
| A larger enclosing expression | `Alt-o`; use `Alt-i` to retrace recent expansions |
| Separate selections for sibling structures | `Alt-a` |
| Separate selections for the contents of a structure | Select the parent, then `Alt-I` |

Try selecting a value in a nested array or call, expanding to its parent, and
then selecting that parent's children. Inspect each selection before editing:
the same keys can work at several levels of the code.

**Previous:** [Editing effectively](../editing-effectively.md) ·
**Next:** [Repeat and combine edits](./repeated-edits.md)

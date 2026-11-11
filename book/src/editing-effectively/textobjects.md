# Edit code with textobjects

A textobject selects a meaningful piece of text: a word, quoted string,
argument, or function. Once it is selected, the same editing commands work on
any of them. This article starts with words and quotes, then uses code structure
for navigation and a change limited to one function.

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
then act. To select an enclosing expression when you do not know its object
type, try `Alt-o` to expand the selection and `Alt-i` to shrink it. See
[Syntax-aware motions](../syntax-aware-motions.md) for more examples.

**Previous:** [Editing effectively](../editing-effectively.md) ·
**Next:** [Repeat and combine edits](./repeated-edits.md)

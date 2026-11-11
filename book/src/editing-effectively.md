# Editing effectively

Once you can open, save, and edit a file, the next step is to make everyday
changes easier. Start with the task: what text needs to change, and how can you
select it? Inspect the selection, then apply the edit.

Open a fresh scratch buffer with `:new` for each exercise. Paste or type the
starting example, then press `Esc` to return to normal mode. Each exercise starts
with one selection; press `,` to keep only the primary selection if needed.
The keys below use the default bindings. As in [Basics](./basics.md), `miw`
means press `m`, then `i`, then `w`.

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
`mi` selects the inside of a [textobject](./textobjects.md); `ma` includes its
surroundings. For example, `ma"` selects the quotes as well as their contents.
Use `mi"` when the quotes should stay in place.

**Try it:** change `ready` to `in review`, keeping the quotes. Then use `u` to
undo that edit and `U` to redo it.

## Review repeated matches one at a time

Sometimes the same word appears several times, but only some occurrences should
change. Start with:

```text
draft draft draft
```

1. Move onto the first `draft`, press `miw`, then `*`. This makes the selected
   word the search pattern; it does not move to another match.
2. Press `Alt-r`, type `ready`, then `Enter` to replace the first selection.
3. Press `n` to inspect the second match. Leave this one alone.
4. Press `n` again to select the third match, then `.` to repeat the replacement.

The result is:

```text
ready draft ready
```

Use `n` to find a match and `.` only when you want to apply the edit there.
Search wraps by default, so stop after reviewing the intended matches.
In Mitos, `.` repeats the last insertion or prompted replacement. It does not
replay every kind of edit or the motions you used to find the text.
See [Macros and undo](./macros.md) for repeating a sequence of commands.

## Change every match in a chosen region

Use multiple selections when all the matches should receive the same edit.
For this example, start with:

```text
user_id = 7
backup_user_id = 8
print(user_id)
```

1. Press `%` to select the whole buffer.
2. Press `s`, type `\buser_id\b`, then `Enter`.
3. Check the highlighted selections. There should be two; `backup_user_id`
   should stay unselected.
4. Press `Alt-r`, type `customer_id`, then `Enter`.

The result is:

```text
customer_id = 7
backup_user_id = 8
print(customer_id)
```

`s` searches within the current selections. To limit a change to a few lines,
select those lines instead of pressing `%`: with one cursor on the first line,
press `3x` to select three lines, for example. You can also use `mip` for a
paragraph or `mif` for a function body when the language has textobject support.

The pattern is a regular expression. `\b` marks a word boundary, so the example
matches `user_id` as a whole word. A dot in a pattern means any character;
use `\.` to match a literal dot. Replacement text entered with `Alt-r` is
literal. See [Search and replacement](./search.md) for more details.

Text matching does not distinguish two variables with the same name. For a
code symbol, prefer `Space r` when your [language server](./lsp.md) supports
renaming: it can identify references to that symbol across files.

## Add a prefix to selected lines

Selections can represent whole lines as well as words. Start with:

```text
TODO write tests
DONE draft guide
TODO update screenshots
```

To turn only the TODO lines into checklist items:

1. Press `%`, then `Alt-s` to create a selection for each line.
2. Press `K`, type `^TODO\b`, then `Enter` to keep only selections starting
   with the word `TODO`.
3. Check that the DONE line is no longer selected.
4. Press `I`, type `- [ ] `, including the trailing space, then `Esc`.

The result is:

```text
- [ ] TODO write tests
DONE draft guide
- [ ] TODO update screenshots
```

`I` inserts before the first nonblank character of each selected line, so it
preserves existing indentation. Use `A` instead when you want to append text
at each line's end. See [Selections](./usage.md) for more ways to build and
filter selections.

## Choose how to repeat work

| What needs to happen? | A useful starting point |
| --- | --- |
| Review matches and change only some | Search with `/` or `*`, move with `n`, repeat an insertion or replacement with `.` |
| Apply the same edit to all matches in a region | Select the region, use `s`, then `c` or `Alt-r` |
| Add text to several lines | Select the lines, split with `Alt-s`, then use `I` or `A` |
| Repeat several commands in order | Record a [macro](./macros.md#record-and-replay-a-macro) with `Q`, stop with `Q`, replay with `q` |
| Rename a code symbol and its references | Use `Space r` with a language server that supports renaming |

For a macro, make the starting position predictable and finish where the next
replay should begin. For example, use a line boundary or a search rather than a
fixed number of left and right movements. Replay once and check the result
before adding a count such as `3q`.

## Explore a project and return to your edit

Open the project from its root directory with `ms .`. Use `Space f` and type
part of a filename to find a file; use `Space /` when you know some text but
do not know which file contains it.

For example, while editing a caller:

1. Press `Ctrl-s` in normal mode to save your location in the jumplist.
   This saves a location, not the file; use `:write` to save text.
2. Put the cursor on a function name and press `gd` to go to its definition
   when a language server provides it. Otherwise, use `Space /` to search for
   the name and press `Enter` on a result.
3. Press `Ctrl-o` to step back through jumps until you return to your edit.
   `Ctrl-i` steps forward again.

Use `ga` to switch between the two most recently accessed buffers. To keep
the caller and definition visible together, press `Ctrl-v` on a picker result
to open it in a vertical split. See [Files, buffers, and splits](./files.md)
and [Jumplist](./jumplist.md).

## Recover, then keep practicing

Before editing, check both the selected text and the number of selections.
`Esc` leaves select or insert mode; it does not remove extra selections.
Press `,` to keep one selection and `;` to collapse it to its cursor.

If you select the wrong text, use `Alt-u` to restore the previous selections
before making an edit. If you change the wrong text, use `u` to undo the edit.
Editing text clears selection history. See [Undo selection changes](./usage.md#undo-selection-changes)
for the distinction.

Pick one exercise, try it on different text, and use it in your next small
editing task. When a task feels repetitive, return to the table above and try
another approach. Keep the default bindings while learning; add a
[custom binding](./remapping.md) when you know which repeated task it will help.

## Further reading

[Learn Vim the Smart Way](https://learnvim.irian.to/) by Igor Irianto and
[Practical Vim](https://pragprog.com/titles/dnvim2/practical-vim-second-edition/)
by Drew Neil offer more ideas for practicing modal editing and reducing
repetitive work. Their examples use Vim commands; use the Mitos workflows on
this page and [Migrating from Vim](./from-vim.md) to apply those ideas here.

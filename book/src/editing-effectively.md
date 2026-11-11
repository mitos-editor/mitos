# Editing effectively

These articles build on [Basics](./basics.md) to help you use Mitos for everyday
work. Start by selecting the text you want to change, learn to repeat that edit,
then use those skills while exploring and reviewing a project.

Follow the articles in order, or choose the one that fits your current task:

| Article | What you will practice |
| --- | --- |
| 1. [Edit code with textobjects](./editing-effectively/textobjects.md) | Change words and strings, move between functions, and limit edits to one function |
| 2. [Repeat and combine edits](./editing-effectively/repeated-edits.md) | Review matches, edit many selections, filter lines, and choose when to use a macro |
| 3. [Navigate a project](./editing-effectively/project-navigation.md) | Find files and symbols, follow code, compare files in splits, and return to your edit |
| 4. [Review and stage changes](./editing-effectively/version-control.md) | Review unsaved edits, stage the intended file version, and use Git history |

The exercises use the default bindings. As in Basics, `miw` means press `m`,
then `i`, then `w`; `Ctrl-o` means press both keys together. Type each exercise
and compare your result with the example before trying it on your own files.

## Recover, then keep practicing

Before editing, check both the selected text and the number of selections.
`Esc` leaves select or insert mode; it does not remove extra selections.
Press `,` to keep one selection and `;` to collapse it to its cursor.

If you select the wrong text, use `Alt-u` to restore the previous selections
before making an edit. If you change the wrong text, use `u` to undo the edit.
Editing text clears selection history. See [Undo selection changes](./usage.md#undo-selection-changes)
for the distinction.

Pick one exercise, try it on different text, and use it in your next small
editing task. When a task feels repetitive, revisit
[Repeat and combine edits](./editing-effectively/repeated-edits.md) for another
approach. Keep the default bindings while learning; add a
[custom binding](./remapping.md) when you know which repeated task it will help.

## Further reading

[Learn Vim the Smart Way](https://learnvim.irian.to/) by Igor Irianto and
[Practical Vim](https://pragprog.com/titles/dnvim2/practical-vim-second-edition/)
by Drew Neil offer more ideas for practicing modal editing and reducing
repetitive work. Their examples use Vim commands; use the Mitos workflows on
this page and [Migrating from Vim](./from-vim.md) to apply those ideas here.

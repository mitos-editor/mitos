# Textobjects

## Selecting and manipulating text with textobjects

In Mitos, textobjects are a way to select, manipulate and operate on a piece of
text in a structured way. They allow you to refer to blocks of text based on
their structure or purpose, such as a word, paragraph, or a
function or block of code.

![Textobject demo](https://user-images.githubusercontent.com/23398472/124231131-81a4bb00-db2d-11eb-9d10-8e577ca7b177.gif)
![Textobject tree-sitter demo](https://user-images.githubusercontent.com/23398472/132537398-2a2e0a54-582b-44ab-a77f-eb818942203d.gif)

- `ma` - Select around the object (`va` in Vim, `<alt-a>` in Kakoune)
- `mi` - Select inside the object (`vi` in Vim, `<alt-i>` in Kakoune)

| Key after `mi` or `ma` | Textobject selected      |
| ---                    | ---                      |
| `w`                    | Word                     |
| `W`                    | WORD                     |
| `p`                    | Paragraph                |
| `(`, `[`, `'`, etc.    | Specified surround pairs |
| `m`                    | The closest surround pair    |
| `f`                    | Function                 |
| `t`                    | Type (or Class)          |
| `a`                    | Argument/parameter       |
| `c`                    | Comment                  |
| `T`                    | Test                     |
| `e`                    | Data structure entry     |
| `g`                    | Change                   |
| `s`                    | Spelling finding         |
| `x`                    | (X)HTML element          |

Function, type, argument, comment, test, entry, and element objects require
syntax support for the language. Check the textobjects column in
[Language support](./lang-support.md).

The `s` textobject uses [spell-checking findings](./spell-checking.md). Both
`mis` and `mas` select the diagnosed word under the cursor; `mIs` and `mAs`
select every finding fully contained in the current selection. Use `[s` and
`]s` to navigate to the previous or next finding. These commands also work in
plain text and scratch buffers when spell checking is enabled.

## Select all objects in a region

Use `mI` for every inside object or `mA` for every around object fully contained
in the current selections. For example, press `%`, then `mIf` to select the
inside of every function in the buffer. Apply `y`, `c`, or another action to
all of them at once.

Select-all supports words, paragraphs, the syntax objects above, Git changes,
and spelling findings. `mId` and `mAd` select diagnostic ranges. Surround pairs
are selected individually with `mi` and `ma`.

## Navigating using tree-sitter textobjects

Navigating between functions, classes, parameters, and other elements is
possible using tree-sitter and textobject queries. For
example to move to the next function use `]f`, to move to previous
type use `[t`, and so on.

![Tree-sitter-nav-demo](https://user-images.githubusercontent.com/23398472/152332550-7dfff043-36a2-4aec-b8f2-77c13eb56d6f.gif)

For the full reference see the [unimpaired](./keymap.md#unimpaired) section of the key bind
documentation.

> 💡 This feature relies on tree-sitter textobjects
> and requires the corresponding query file to work properly.

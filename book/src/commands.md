# Commands

The generated keybinding tables use `<space>` for Space, `<ret>` for Enter,
and `<C-x>` for Ctrl-x. Adjacent keys form a sequence: `<space>f` means
press Space, then f.

## Typable commands

Press `:` to run these commands. Some accept arguments; see
[Command mode syntax](./command-line.md) for quoting, flags, and expansions.

{{#include ./generated/typable-cmd.md}}

### Sorting selected text

`:sort` reorders the contents of multiple selections. Select lines with `%` and
split them with `Alt-s` to sort a document by line. Text between selections stays
in place. The default is case-sensitive lexicographic order.

| Option | Effect |
| --- | --- |
| `--reverse`, `-r` | Sort in descending order. Equal keys keep their original order. |
| `--insensitive`, `-i` | Compare lowercase keys. Original text is preserved. |
| `--numeric`, `-n` | Compare leading decimal numbers exactly, without integer limits or floating-point rounding. |
| `--natural`, `-N` | Compare ASCII digit runs as numbers: `file2` comes before `file10`. |
| `--length`, `-l` | Compare Unicode character counts, rather than bytes or displayed width. Combining marks count separately. |
| `--ignore-leading-blanks`, `-b` | Ignore leading spaces and tabs in the comparison key. |
| `--key REGEX`, `-k REGEX` | Use the first complete regex match as the comparison key. Selections without a match stay in their original slots. |

Numeric, natural and length modes are mutually exclusive. Other options can be
combined with any mode. Keys are extracted first, then leading blanks are
removed if requested. Equal keys retain their input order in both directions;
there is no implicit comparison of the rest of the selected text.
`--insensitive` affects comparisons, not regex matching; put `(?i)` in the
pattern to match without case sensitivity.

Numeric mode skips leading whitespace and accepts an optional minus sign,
ASCII digits, and a decimal fraction. Missing numbers count as zero, including text
starting with `+`. Exponents and number-base prefixes are not interpreted:
`1e3` has the numeric key `1`. Negative zero equals zero. Natural mode treats
signs and punctuation as text and compares unsigned digit runs anywhere in the
key; leading zeroes do not break ties (`file02` and `file2` have equal keys).

Examples, after splitting the desired text into selections:

```text
:sort --numeric
:sort --natural --insensitive
:sort --length --reverse
:sort --key '\d+' --numeric
:sort --key '[^,]+$' --insensitive
```

The fourth example sorts by the first run of digits while keeping headers or
other selections without digits in place. The last sorts by the final nonempty
comma-separated field. Quote patterns using the normal
[command mode quoting rules](./command-line.md).

## Static Commands

Static commands take no arguments. Run them from the command palette (`Space ?`)
or use their names in [keybindings](./remapping.md).

{{#include ./generated/static-cmd.md}}

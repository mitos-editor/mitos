# EditorConfig

Mitos reads `.editorconfig` files by default. Use them to share file conventions
with other editors, or apply settings to paths rather than entire languages.

For example, save this as `.editorconfig` in a project:

```ini
root = true

[*]
indent_style = space
indent_size = 4
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true

[*.md]
max_line_length = 80
spelling_language = en_US
```

Mitos reads matching sections from the file's directory and its parents.
Closer files override settings from higher directories; later matching sections
in a file override earlier ones. `root = true` stops the search for parent
configuration. Use `unset` to remove an inherited property.

## Supported properties

| Property | Values and behavior |
| --- | --- |
| `indent_style` | `space` or `tab` |
| `indent_size` | Number of spaces, or `tab` to use `tab_width`; indentation is limited to 1–16 columns |
| `tab_width` | Width used to display tabs |
| `end_of_line` | `lf` or `crlf`; `cr` also works in builds with Unicode line endings |
| `charset` | `utf-8`, `latin1`, `utf-16le`, or `utf-16be`; `utf-8-bom` is ignored |
| `insert_final_newline` | `true` or `false` |
| `trim_trailing_whitespace` | `true` or `false` |
| `max_line_length` | Positive integer; sets the text width used for reflow and optional wrapping |
| `spelling_language` | A dictionary name, such as `en_US`; see [Spell checking](./spell-checking.md) |

The last two properties are extensions to the standard EditorConfig properties.
Unknown properties are ignored.

## Precedence

Matching EditorConfig settings take precedence over language and editor defaults
for indentation, line endings, encoding on open, save cleanup, and text width.
Where a property is absent, Mitos uses its usual defaults and file detection.
An explicit `:set-spelling-language` choice overrides `spelling_language` for
that buffer.

To ignore EditorConfig files, add this to `config.toml`:

```toml
[editor.files]
editor-config = false
```

See [Languages](./languages.md) for settings specific to a language and
[Configure Mitos](./editor.md) for your own preferences.

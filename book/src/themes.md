# Themes

To use a theme add `theme = "<name>"` to the top of your [`config.toml`](./configuration.md) file, or select it during runtime using `:theme <name>`.

Without a configured theme, Mitos uses `modus_operandi` for light mode and
`modus_vivendi` for dark mode. If the terminal does not report a preference,
Mitos uses `modus_vivendi`. Terminals without true-color support use the
built-in ANSI `default` theme instead. To always use the terminal's ANSI
palette, set `theme = "default"`.

Separate themes can be configured for light and dark modes. On terminals supporting [mode 2031 dark/light detection](https://github.com/contour-terminal/contour/blob/master/docs/vt-extensions/color-palette-update-notifications.md), the theme mode is detected from the terminal and updates when the terminal follows a system appearance change. On other terminals, Mitos uses the configured `fallback` theme, or the dark theme when no fallback is specified.

```toml
[theme]
dark = "modus_vivendi"
light = "modus_operandi"
## Optional. Used if the terminal doesn't declare a preference.
## Defaults to the theme set for `dark` if not specified.
# fallback = "modus_vivendi"
```

## Creating a theme

Create a file with the name of your theme as the file name (i.e `mytheme.toml`) and place it in your `themes` directory (i.e `~/.config/mitos/themes` or `%AppData%\mitos\themes` on Windows). The directory might have to be created beforehand.

> 💡 The names "default" and "base16_default" are reserved for built-in themes
> and cannot be overridden by user-defined themes.

### Overview

Each line in the theme file is specified as below:

```toml
key = { fg = "#ffffff", bg = "#000000", underline = { color = "#ff0000", style = "curl"}, modifiers = ["bold", "italic"] }
```

Where `key` represents what you want to style, `fg` specifies the foreground color, `bg` the background color, `underline` the underline `style`/`color`, and `modifiers` is a list of style modifiers. `bg`, `underline` and `modifiers` can be omitted to defer to the defaults.

To specify only the foreground color:

```toml
# Valid hex-color format is `#RGB` or `#RRGGBB`
# (each letter is a nibble)
key = "#ffffff"
```

If the key contains a dot `'.'`, it must be quoted to prevent it being parsed as a [dotted key](https://toml.io/en/v1.0.0#keys).

```toml
"key.key" = "#fff"
```

Color values must be either a [CSS hex RGB string](https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Values/hex-color) or a name declared in the [`palette`](#color-palettes).

> 💡 Note that Mitos doesn't support transparency (alpha channel).

For inspiration, you can find the built-in ANSI theme `base16_terminal.toml`
[here](https://github.com/mitos-editor/mitos/blob/main/runtime/themes/base16_terminal.toml) and
user-submitted themes
[here](https://github.com/mitos-editor/mitos/blob/main/runtime/themes).

### Panel and popup borders

Use `ui.border` to style panel and popup borders, including file pickers and
documentation hovers, independently of their text:

```toml
"ui.text" = "#d8dee9"
"ui.border" = "#4c566a"
```

If `ui.border` is omitted, borders use `ui.text`. Split separators use
`ui.window`; internal separators retain their own theme scopes.
Border shapes and visibility are controlled by the
[border configuration](./configuration.md#popup-borders).


## The details of theme creation

### Color palettes

It's recommended to define a palette of named colors, and refer to them in the
configuration values in your theme. To do this, add a table called
`palette` to your theme file:

```toml
"ui.background" = "white"
"ui.text" = "black"

[palette]
white = "#ffffff"
black = "#000000"
```

Keep in mind that the `[palette]` table includes all keys after its header,
so it should be defined after the normal theme options.

The default palette uses the terminal's default 16 colors, and the colors names
are listed below. The `[palette]` section in the config file takes precedence
over it and is merged into the default palette.

| Color Name      |
| ---             |
| `default`       |
| `black`         |
| `red`           |
| `green`         |
| `yellow`        |
| `blue`          |
| `magenta`       |
| `cyan`          |
| `gray`          |
| `light-red`     |
| `light-green`   |
| `light-yellow`  |
| `light-blue`    |
| `light-magenta` |
| `light-cyan`    |
| `light-gray`    |
| `white`         |

### Modifiers

The following values may be used as modifier, provided they are supported by
your terminal emulator.

| Modifier             |
| ---                  |
| `bold`               |
| `dim`                |
| `italic`             |
| `underlined`         |
| `slow_blink`         |
| `rapid_blink`        |
| `reversed`           |
| `hidden`             |
| `crossed_out`        |

> 💡 The `underlined` modifier is deprecated and only available for backwards compatibility.
> Its behavior is equivalent to setting `underline.style="line"`.

### Underline style

One of the following values may be used as a value for `underline.style`, providing it is
supported by your terminal emulator.

| Modifier       |
| ---            |
| `none`         |
| `line`         |
| `curl`         |
| `dashed`       |
| `dotted`       |
| `double_line`  |


### Inheritance

Extend other themes by setting the `inherits` property to an existing theme.

```toml
inherits = "modus_vivendi"

# Override the theming for "keyword"s:
"keyword" = { fg = "gold" }

# Override colors in the palette:
[palette]
gold = "#D4AF37"
```

### Rainbow

The `rainbow` key is used for rainbow highlight for matching brackets.
The key is a list of styles.

```toml
rainbow = ["#ff0000", "#ffa500", "#fff000", { fg = "#00ff00", modifiers = ["bold"] }]
```

Colors from the palette and modifiers may be used.

### Scopes

Syntax queries assign a **kind**, such as `function.method`, and optional
**qualifiers**, such as `definition`, `public`, or `deprecated`. Kinds form a
hierarchy; qualifiers describe independent roles or traits. For example,
`function.method.public.definition` identifies a public method definition.
Qualifier order does not affect theme matching.

Definitions, declarations, calls, and references are distinct:

| Scope | Meaning |
| --- | --- |
| `function.definition` | Function definition |
| `function.declaration` | Function declaration/signature without a known definition body |
| `function.call` | Function invocation |
| `function.reference` | Reference to a function value |
| `function.method.definition` | Method definition |
| `function.method.declaration` | Method declaration/signature without a known definition body |
| `function.method.call` | Method invocation |
| `function.method` | Method with an unspecified occurrence role |

A query may use a broader scope when syntax alone cannot establish the kind or
role. Calling `pkg.name()` does not necessarily establish whether `pkg` is a
package or a value. Passing a callable as an argument does not invoke it.
Language coverage varies with each grammar and query.

Style syntax with ordinary theme keys. Use `*` in place of the kind to match a
qualifier across kinds:

```toml
"function" = "blue"
"function.method" = "cyan"
"*.definition" = { modifiers = ["bold"] }
"*.deprecated" = { modifiers = ["crossed_out"] }
"*.documentation" = { modifiers = ["italic"] }
"function.method.public" = { modifiers = ["underlined"] }
"function.method.call" = "green"
```

These rules make public method definitions cyan, bold, and underlined; method
calls green; and deprecated symbols crossed out. Both
`function.method.public.definition` and `function.method.definition.public`
match the public-method and definition rules.

Matching syntax rules are applied from broad to specific. Specificity compares
kind depth first, then the number of qualifiers. Equally specific rules apply
in lexical order, with the later rule winning conflicting properties. An exact
combination can resolve a tie explicitly. Properties are merged: a bold-only
rule keeps the inherited foreground. More specific colors and underline
properties override broader ones; style modifiers combine. Use
`remove-modifiers = ["bold"]` to remove an inherited modifier, or
`underline = { style = "none" }` to remove inherited underlining.

This property inheritance applies to syntax styles, including `markup` and
`diff` syntax kinds. Interface keys keep their existing longest-prefix fallback
and do not merge parent properties. Theme-file `inherits` continues to replace
an explicitly overridden key, while merging palettes.

Existing keys such as `function.public`, `function.method.private`,
`variable.parameter.mutable`, and `comment.block.documentation` remain valid.
Custom scopes are accepted at runtime and retain prefix fallback. Bundled
queries and themes must use the public vocabulary below; the generated capture
inventory ensures every bundled classification can resolve composed rules.

### Semantic highlighting

When enabled, a language server can refine syntax highlighting with resolved
symbol kinds and traits, such as `type.class`, `variable.other.member.readonly`,
or `function.method.definition.deprecated`. Syntax queries and semantic tokens
use the same theme selectors. Semantic styles are applied over syntax colors,
before selections, search matches, and diagnostics. An unspecified semantic
occurrence role preserves a call or reference already established by syntax.
Explicit semantic roles take precedence, except that a generic declaration
flag preserves a definition body established by syntax. Servers often mark
both declarations and definitions with that flag. Unstyled properties retain
the underlying syntax appearance.

Semantic highlighting is enabled by default. To disable it:

```toml
[editor.lsp.semantic-tokens]
enable = false
```

A language can exclude a particular server from semantic highlighting using
`except-features = ["semantic-tokens"]` in its `language-servers` entry. If
several servers support the feature, Mitos uses the first configured provider.
Full-document and range-only providers are supported. Requests are debounced
after edits; stale tokens are cleared immediately, and responses are discarded
when their document snapshot or server binding has changed. Unsupported server
extensions fall back to syntax highlighting. Delta updates and dynamic
semantic-token registration are not advertised.

Use `variable.readonly` and `variable.other.member.readonly` to style read-only
bindings and fields. Bundled themes define these rules explicitly.

A semantic token without a declaration or definition modifier is not assumed to
be a call: it may be a reference to a callable value.

### Independent interface and annotation colors

Use `ui.picker.match` for matching text in pickers and the file explorer.
Welcome-screen headings and command examples use `ui.welcome.heading` and
`ui.welcome.command`. Use `comment.error`, `comment.warning`, `comment.info`,
and `comment.hint` for annotations in comments. These settings let source
annotations and interface matches differ from diagnostics and language syntax.

Inline diagnostic text uses `ui.diagnostic.inline.error`, `.warning`, `.info`,
and `.hint`. Diagnostic underlines in the editing area use `diagnostic.error`,
`diagnostic.warning`, and related keys.

To keep syntax colors visible in review buffers, give `diff.plus.word` and
`diff.minus.word` a background or text modifier without setting a foreground.

### Inspecting and validating scopes

Run `:highlight-inspect` to see syntax classifications, semantic refinement,
matching theme selectors, and resolved styles at the cursor. `:tree-sitter-scopes` shows syntax-tree node
names instead.

For contributors, `cargo xtask scope-check` checks bundled theme keys and query
captures against the registry. `cargo xtask theme-check` checks style values
and palettes. Run `cargo xtask docgen` after changing the registry or captures
to regenerate the reference and capture inventory.

### Scope reference

The reference is generated from `crates/theme/src/scopes/`. Syntax kinds are
inspired by [Helix](https://docs.helix-editor.com/themes.html) and
[Neovim](https://neovim.io/doc/user/treesitter/#treesitter-highlight-groups);
independent roles and traits also map the standard
[LSP semantic classifications](https://code.visualstudio.com/api/language-extensions/semantic-highlight-guide).

{{#include generated/theme-scopes.md}}

[editor-section]: ./configuration.md#editor

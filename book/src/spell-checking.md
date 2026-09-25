# Spell checking

Mitos can spell-check documents and surface misspellings as diagnostics.
Corrections and "add to dictionary" are offered as [code
actions](./commands.md), alongside any from language servers.

Spell checking is opt-in. Mitos will use tree-sitter to check only requested
regions of a language to avoid noisy diagnostics for keywords. Multiple
dictionaries may be configured at once. A word is accepted if any configured
dictionary allows it.

## Enabling

There are several ways to turn spell checking on:

1. Per buffer, with the command:

   ```
   :set-spelling-language <language>
   ```

   Pass several languages to check against all of them (`:set-spelling-language
   en_US en_GB`), or `off` to disable checking for the buffer. With no argument
   it reports the current language(s). This choice overrides the settings below
   and persists across config reloads for that buffer.

2. Per path, with an [`.editorconfig`](https://editorconfig.org/)
   `spelling_language` key:

   ```ini
   [*.md]
   spelling_language = en_US
   ```

3. Per language, in `languages.toml` (see [Languages](./languages.md)):

   ```toml
   [[language]]
   name = "markdown"
   spelling.languages = ["en_US"]
   ```

4. Globally, in `config.toml` under [`[editor.spelling]`](./configuration.md#editorspelling):

   ```toml
   [editor.spelling]
   languages = ["en_US"]
   ```

## Scope

What gets checked is controlled per language by a tree-sitter `spellcheck.scm`
query: a node captured `@spell` is checked, and `@nospell` excludes part of one.
Most languages inject a shared `comment` grammar, so their comments are checked
with no language-specific query. The bundled queries also cover:

| Language | Checked text | Excluded text |
| --- | --- | --- |
| Markdown | Prose, headings, lists, and visible link text | Inline code and link destinations |
| Python | Module, class, and function docstrings, including after leading comments | Ordinary strings, bytes, f-strings, and escape sequences |
| HTML | Visible text, including headings and link labels | Tags, attributes, entities, code elements (`code`, `pre`, `kbd`, `samp`), scripts, and styles |
| JSX / TSX | JSX text, including nested JSX inside expressions | Component names, attributes, expressions, and code elements |
| Git commit messages | Subject, body, and breaking-change descriptions | Conventional prefixes, trailers, template comments, and diffs |
| reStructuredText | Headings, paragraphs, lists, and quotations | Literal code, interpreted roles, references, and directive bodies |
| Typst | Prose, headings, emphasis, and content blocks | Code, ordinary strings, math, labels, references, URLs, and escapes |

Python docstrings are treated as prose; examples and markup inside them are
not parsed separately. reStructuredText directive bodies are left unchecked
because their syntax nodes do not distinguish prose from code.

Coverage depends on the queries present in your runtime directories. Strings
and identifiers are checked only where a language's query opts them in. With
syntax parsing, text outside `@spell` captures is left unchecked. Plain text
and files without a syntax tree are checked in full.

See [Adding spellcheck queries](./guides/spellcheck.md) to extend coverage to a
new language.

## Navigating and selecting findings

Use `]s` to select the next spelling finding and `[s` to select the previous
one. These commands skip other diagnostics, support counts such as `3]s`, and
stop at the first or last finding without wrapping. In select mode they extend
the selection; with multiple cursors each cursor moves independently.

The `s` [textobject](./textobjects.md) selects the diagnosed word under the
cursor with `mis` or `mas`. Use `mIs` or `mAs` to select all spelling findings
fully contained in the current selection. Both inside and around variants use
the finding's exact range.

## Correcting findings

Move onto a misspelled word (or select it with `]s`, `[s`, or `mis`) and press
`Space-a` to open code actions. Choose a `Replace ...` action and press `Enter`
to replace the whole word. Press `u` to undo the correction, or `Escape` to
dismiss the menu without changing the text.

Spelling actions work without a language server. With multiple selections,
the menu offers corrections for findings overlapping the primary selection.
The same menu offers `Add ... to dictionary` to accept a word permanently.

### Ignoring a word for this session

Choose `Ignore 'word' for this session (language)` from `Space-a` to suppress
that word until Mitos exits. Matching is case-insensitive and applies to the
whole word, including in other open buffers and buffers opened later that use
the chosen dictionary. Other misspellings continue to be checked.

With multiple dictionaries, choose the language the ignore should apply to.
A buffer ignores the word if any of its configured dictionaries has a session
ignore for it. The ignore survives edits, configuration reloads, and toggling
spell checking off and back on. It does not change the document or write to
your personal dictionary or configuration files.

### Ignoring a word permanently

Choose `Ignore 'word' forever (language)` from `Space-a` to keep ignoring it
after restarting Mitos, across projects using that dictionary. Matching is
case-insensitive and applies to the whole word, just like a session ignore.

Persistent ignores are stored as UTF-8 text, one word per line, in
`<config>/spelling/<language>.ignore`, for example
`~/.config/mitos/spelling/en_US.ignore` on Linux. They are loaded when the
dictionary is first used. Remove a word from that file and restart Mitos to
check it again, or add words there by hand.

`Ignore forever` suppresses findings without adding words to the dictionary's
suggestion vocabulary. Use `Add ... to dictionary` when you also want the word
available as a correction suggestion. A failed save reports an error and
leaves the word's checking behavior unchanged.

## Dictionaries

Dictionaries are Hunspell `.aff`/`.dic` pairs loaded from
`dictionaries/<language>/<language>.{aff,dic}` in the [runtime
directories](./building-from-source.md#configuring-mitoss-runtime-files). `ms --health`
lists those directories.

Mitos bundles only `en_US`. To add another language, drop its Hunspell files
into a runtime directory, named after the language code you reference in the
config. For example, for `de_DE`:

```
~/.config/mitos/runtime/dictionaries/de_DE/de_DE.aff
~/.config/mitos/runtime/dictionaries/de_DE/de_DE.dic
```

Hunspell dictionaries for most languages are distributed with LibreOffice and
by the various aspell/Hunspell projects.

### Personal dictionary

The "Add to dictionary" code action accepts a word permanently. It is appended
to a personal dictionary, one word per line, under Mitos's state directory:

```
<state>/dictionaries/<language>.txt
```

(`<state>` is e.g. `~/.local/state/mitos` on Linux.) The file is namespaced per
language, so a word added for one language is not accepted in another. You can
edit it by hand; entries are loaded the next time that language's dictionary is
read.

## Tuning

Spell checking produces false positives on names, jargon, and code-like tokens.
URLs and email addresses are skipped automatically. Beyond that, three knobs
(settable globally under `[editor.spelling]` and per language in
`languages.toml`) reduce noise:

| Key               | Description
| ---               | ---
| `words`           | Extra accepted words, matched case-insensitively.
| `ignore-regexes`  | Tokens matching any of these are not checked (e.g. `"^[A-Z0-9_]+$"`).
| `min-word-length` | Tokens shorter than this are not checked.

Identifiers are checked word by word: `snake_case`, `camelCase`, `PascalCase`,
and `HTTPServer` are split at word boundaries. Underscores, hyphens, and digits
separate words; numbers alone are skipped. For example, `hello_wrld` reports
only `wrld`, and a correction preserves the `hello_` prefix. Apostrophes and
Unicode combining marks stay attached to their words.

The complete token is checked against ignores and dictionaries first, preserving
explicitly accepted identifiers and hyphenated words. Otherwise, the settings
above also apply to each word within it, including `min-word-length`.

When both global and per-language settings are present, `languages` and
`min-word-length` are replaced by the language's value, while `words` and
`ignore-regexes` are added to the global lists. See
[`[editor.spelling]`](./configuration.md#editorspelling) for the full
reference.

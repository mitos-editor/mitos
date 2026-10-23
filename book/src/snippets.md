# User-defined snippets

Snippets insert reusable text with placeholders you can fill in. They appear in
[completion](./completion.md) without a language server, and you can insert them
from a picker or a key binding.

## Create a snippet

Run `:snippets-open` to open `snippets.toml` in your Mitos configuration directory.
Add a definition, save the file, and run `:snippets-reload`:

```toml
[[snippet]]
name = "Error check"
prefix = ["err", "iferr"]
scope = "go"
description = "Return an error from the current function"
body = '''if ${1:err} != nil {
    return $1
}
$0'''
```

In a Go file, type `err` and accept the snippet from the completion menu. Replace
the selected `err`, then use [placeholder navigation](./completion.md#snippet-placeholders)
to finish editing.

| Field | Meaning |
| --- | --- |
| `name` | Required, unique name used in the picker and `:snippet-insert`. |
| `body` | Required, nonempty snippet text, or an array of lines joined with newlines. |
| `prefix` | Trigger text or a list of triggers. Omit to use only the picker or `:snippet-insert`. |
| `scope` | Language name from `languages.toml`, or a list of names. Omit to use in every language, including plain text. |
| `description` | Optional explanation shown in completion and the picker. |

For example, the body above can also be written as:

```toml
body = ["if ${1:err} != nil {", "    return $1", "}", "$0"]
```

Prefixes may contain punctuation, such as `for-const` or `for!`, but no whitespace
or control characters. Press `Ctrl-x` to request completion after punctuation;
automatic completion is triggered by word characters. Accepting a snippet
replaces its matching prefix at each cursor. Different snippets may share a prefix.

Language scopes follow embedded languages at the cursor when syntax information
is available. To disable user snippet suggestions, set
`editor.completion.snippets = false`. The picker and named insertion remain
available. Language-server snippets have a separate setting,
`editor.lsp.completion.snippets`.

## Organize snippets in several files

Keep definitions in `snippets.toml`, split them across `.toml` files in the
sibling `snippets/` directory, or use both:

```text
~/.config/mitos/
  snippets.toml
  snippets/
    rust.toml
    go/
      errors.toml
      http.toml
```

Each file uses the same `[[snippet]]` format and may contain several definitions.
Subdirectories are included. Filenames only organize the collection: set `scope`
in each definition to choose its languages. Names must be unique across all
user snippet files. Duplicate-name errors identify both files.

Run `:snippets-open go/http.toml` to open a file in `snippets/`. Mitos creates
missing parent directories; save the file to create it. The filename must be a
relative `.toml` path. Other extensions and directory symlinks are ignored when
loading collections; symlinks to regular files are supported.

After adding, editing, renaming, or deleting files, run `:snippets-reload`.
`:config-reload` also reloads snippets. If any file contains an error, the reload
reports it and your previous snippets remain available.

## Share snippets with a project

Put project definitions in `.mitos/snippets.toml` or `.toml` files under
`.mitos/snippets/` at the workspace root. They use the same format and become
available after you [trust the workspace](./workspace-trust.md).

Names must be unique within the project collection. A project snippet with the
same name as a user snippet replaces it, including its triggers and language
scope. Removing the project definition and reloading restores the user snippet.
Other user snippets remain available. Each workspace uses its own project snippets.

Use `:snippets-open-workspace` to edit the project's primary file, or
`:snippets-open-workspace go/http.toml` to edit a file in its snippet directory.
Save and reload as above. Changes to trusted project settings may require you to
renew workspace trust.

## Browse and insert snippets

Bind `snippet_picker` to browse snippets available in the current language:

```toml
[keys.normal]
A-s = "snippet_picker"
```

The picker shows names, triggers, descriptions, and source files. Select a
snippet and press `Enter` to insert it. If Mitos asks you to reopen the picker,
choose the snippet again.

To bind a particular snippet directly, use its exact name:

```toml
[keys.normal]
A-e = ':snippet-insert "Error check"'
```

Insertion replaces selected text in select mode and enters insert mode with the
first placeholder selected. Larger selections and active placeholders are also
replaced in other modes. A single insert cursor inserts before the character
under it. Use `$TM_SELECTED_TEXT` to wrap selected text:

```toml
[[snippet]]
name = "Parenthesize selection"
body = '(${TM_SELECTED_TEXT})$0'
```

| Command | Action |
| --- | --- |
| `snippet_picker` | Browse and insert snippets for the current language. |
| `:snippet-insert <name>` | Insert a snippet by its exact name. |
| `:snippets-open [file]` | Open the user snippet file, or a named file in `snippets/`. |
| `:snippets-open-workspace [file]` | Open the project snippet file, or a named file in `.mitos/snippets/`. |
| `:snippets-reload` | Reload user and open workspace snippets. |

## Write snippet bodies

Bodies use the same syntax as language-server snippets:

| Syntax | Effect |
| --- | --- |
| `$1`, `$2` | Editable positions, visited in number order. Repeated numbers are edited together. |
| `${1:default}` | A placeholder with selected default text. Defaults may contain other placeholders. |
| `$0` | Final cursor position after the last placeholder. |
| `${1|one,two,three|}` | A choice placeholder, initially containing `one`. |
| `$NAME`, `${NAME:default}` | Insert a [variable](#variables), with an optional fallback. |
| `${NAME/regex/replacement/}` | Transform a variable before inserting it. |

Use [placeholder navigation](./completion.md#snippet-placeholders) to move
forward and backward. For choice placeholders, type a replacement or bind
commands to cycle the available values:

```toml
[keys.insert]
A-n = "next_snippet_choice"
A-p = "prev_snippet_choice"
```

Use TOML literal strings (`'...'` or `'''...'''`) to avoid a second layer of
backslash escaping. Write `\$` for a literal dollar sign, `\}` for a literal
closing brace, and `\\` for a literal backslash. In a choice, escape a literal
comma or pipe with `\,` or `\|`. Use actual tabs or spaces for indentation in
literal strings; Mitos adapts indentation to the document.

For example, this inserts the filename without its final extension in uppercase:

```toml
body = '${TM_FILENAME/(.*)\..+$/${1:/upcase}/}'
```

Transforms use Rust regular expressions, with `g` (all matches), `i` (ignore
case), and `m` (multiline) flags. Regex features from other editors, such as
lookaround and backreferences in the pattern, are not supported. Snippet bodies
contain text and do not execute shell commands or other code. Regex transforms
of editable placeholders, such as `${1/regex/replacement/}`, are not supported.

## Variables

These names work in both user-defined and language-server snippets, regardless
of programming language. Comment variables use the language at each cursor.
Text and cursor variables use each cursor's text and selection.

Unknown or unavailable variables insert their fallback, or an empty string when
no fallback is given. For example, `${TM_FILENAME:untitled}` inserts `untitled`
in an unsaved buffer. A transform receives an empty string when its variable is
unavailable. A single insert cursor does not count as selected text; use select
mode to wrap one character.

### Text, files, and workspace

| Variable | Value |
| --- | --- |
| `TM_SELECTED_TEXT`, `SELECTION` | Selected text. |
| `TM_CURRENT_LINE` | Current line without its line ending. |
| `TM_CURRENT_WORD` | Word at the cursor. |
| `TM_LINE_INDEX`, `TM_LINE_NUMBER` | Current line number, starting at 0 or 1 respectively. |
| `CURSOR_INDEX`, `CURSOR_NUMBER` | Cursor number, starting at 0 or 1 respectively. |
| `TM_FILENAME` | Filename, including its extension. |
| `TM_FILENAME_BASE` | Filename without its final extension. |
| `TM_DIRECTORY` | Full path of the file's parent directory. |
| `TM_FILEPATH` | Full path of the file. |
| `RELATIVE_FILEPATH` | File path relative to the workspace root. |
| `WORKSPACE_NAME` | Workspace directory name. |
| `WORKSPACE_FOLDER` | Full path of the workspace root. |
| `CLIPBOARD` | System clipboard contents, when available. |
| `LINE_COMMENT` | Line comment marker, such as `//` in Rust. |
| `BLOCK_COMMENT_START`, `BLOCK_COMMENT_END` | Block comment markers, such as `/*` and `*/` in Rust. |

### Date and time

Dates use local time. Month and weekday names are English.

| Variable | Value or example |
| --- | --- |
| `CURRENT_YEAR`, `CURRENT_YEAR_SHORT` | Year, such as `2026` or `26`. |
| `CURRENT_MONTH` | Two-digit month, such as `02`. |
| `CURRENT_MONTH_NAME`, `CURRENT_MONTH_NAME_SHORT` | Month name, such as `July` or `Jul`. |
| `CURRENT_DATE` | Two-digit day of the month, such as `08`. |
| `CURRENT_DAY_NAME`, `CURRENT_DAY_NAME_SHORT` | Weekday name, such as `Monday` or `Mon`. |
| `CURRENT_HOUR` | Two-digit hour in the 24-hour clock. |
| `CURRENT_MINUTE`, `CURRENT_SECOND` | Two-digit minute or second. |
| `CURRENT_MILLISECOND` | Three-digit millisecond, such as `078`. |
| `CURRENT_SECONDS_UNIX` | Seconds since the Unix epoch. |
| `CURRENT_MILLISECONDS_UNIX` | Milliseconds since the Unix epoch. |
| `CURRENT_TIMEZONE_OFFSET` | UTC offset, such as `+02:00` or `-07:00`. |
| `CURRENT_TIMEZONE_NAME` | IANA time zone name, when available, such as `Europe/Berlin`. |

### Random values

| Variable | Value |
| --- | --- |
| `RANDOM` | Six random decimal digits. |
| `RANDOM_HEX` | Six random hexadecimal digits. |
| `UUID` | A version 4 UUID. |

Each cursor gets its own random values. Repeated references within one snippet
use the same value. For user snippet completions, the preview shows the values
that will be inserted when you accept the suggestion.

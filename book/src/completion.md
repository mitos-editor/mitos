# Completion and snippets

Mitos suggests words from open buffers and filesystem paths while you type.
Install a [language server](./lsp.md) for code completion and LSP snippets, or
define your own [native snippets](#user-defined-snippets).

## Use the completion menu

Press `Ctrl-x` in insert mode to request completion yourself. When the menu
is open:

| Key | Action |
| --- | --- |
| `Tab`, `Ctrl-n`, `Down` | Select the next suggestion |
| `Shift-Tab`, `Ctrl-p`, `Up` | Select the previous suggestion |
| `Enter` | Accept the selected suggestion |
| `Ctrl-c` | Dismiss the menu and reject the preview |

Typing another character normally accepts the selected suggestion and continues
typing. Mitos previews the selected suggestion in the document by default.
Disable this with `editor.completion.preview-insert = false`.

To request suggestions only with `Ctrl-x`:

```toml
[editor.completion]
auto-trigger = false
```

See the [completion settings](./configuration.md#editorcompletion) for delays,
word completion, and path completion. Remap menu keys under
[`keys.menu`](./remapping.md#component-keys).

## Snippet placeholders

Language servers can return snippets with editable placeholders, such as a
function call's arguments. After accepting one, type over the selected
placeholder and press `Tab` to move to the next one when smart-tab is active
and non-whitespace precedes the cursor.

You can bind placeholder navigation directly, including backward navigation:

```toml
[keys.insert]
A-j = "goto_next_tabstop"
A-k = "goto_prev_tabstop"
```

LSP snippets are enabled by default. To disable them, set
`editor.lsp.completion.snippets = false`, reload configuration, and run
`:lsp-restart`.

## Smart Tab

Outside a snippet, `Tab` moves to the end of the enclosing syntax node when
there is non-whitespace text before the cursor. At the start of a line it
inserts indentation. `Shift-Tab` explicitly inserts indentation with the
default bindings.

While the completion menu is open, `Tab` normally selects the next suggestion.
Set `editor.indent.smart-tab.supersede-menu = true` to give smart-tab priority.
You can still select suggestions with `Ctrl-n` and `Ctrl-p`.

To always insert indentation when the menu is closed:

```toml
[editor.indent.smart-tab]
enable = false
```

## Inline completions

Servers supporting inline completion can show suggestions as faint text after
the cursor. These suggestions leave the document unchanged until accepted.
They disappear when you edit, move, leave insert mode, or switch documents.
Inline suggestions pause while you preview an item in the completion menu.

There are no default keys for inline suggestions. Add these to `config.toml`:

```toml
[keys.insert]
C-y = "inline_completion_accept"
C-e = "inline_completion_dismiss"
C-g = "inline_completion_trigger"
A-n = "inline_completion_next"
A-p = "inline_completion_prev"
```

To request suggestions only with your trigger binding:

```toml
[editor.inline-completion]
auto-trigger = false
```

Mitos uses the first configured server with the `inline-completion` feature.
Suggestions can span multiple lines; inline snippet suggestions and multiple
cursors are not supported. See [feature routing](./languages.md#feature-routing)
if you use several servers.

## Snippet variables

Snippet bodies can use `$NAME` or `${NAME:default}`. Mitos resolves text variables
against each cursor's original text and selection, including in completion
previews. A single cursor in insert mode does not count as selected text.

Supported document variables are `TM_SELECTED_TEXT` (also `SELECTION`),
`TM_CURRENT_LINE`, `TM_CURRENT_WORD`, `TM_LINE_INDEX` (zero based),
`TM_LINE_NUMBER` (one based), `CURSOR_INDEX`, and `CURSOR_NUMBER`.
File variables are `TM_FILENAME`, `TM_FILENAME_BASE`, `TM_DIRECTORY`,
`TM_FILEPATH`, and `RELATIVE_FILEPATH`. Workspace variables are `WORKSPACE_NAME`
and `WORKSPACE_FOLDER`. `LINE_COMMENT`, `BLOCK_COMMENT_START`, and
`BLOCK_COMMENT_END` use the document's language configuration.

Unavailable variables use their default, or an empty string if none is given.
Variable transforms such as `${TM_FILENAME/(.*)\..+$/$1/}` use the resolved
value; an unavailable value is transformed as an empty string.

## User-defined snippets

Define snippets in `snippets.toml` in your Mitos configuration directory. They
appear in the completion menu without a language server. For example:

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

`prefix` is a word or a list of words used to find the snippet. Prefixes contain
letters, digits, or underscores, including Unicode word characters. Omit
`prefix` to make a snippet available only through the picker or a named command.
`scope` is a language name from `languages.toml`, or a list of names. Omit it to make a
snippet available in every language, including plain text. Language scopes follow
embedded languages at the cursor when syntax information is available. `name`
and a nonempty `body` are required; `description` is optional. Names must be unique
within each file.
Bodies use the same placeholder and variable syntax as language-server snippets.
Use actual tab characters or spaces for indentation in TOML literal strings.

Save the file and run `:config-reload`. Invalid definitions report the file and
snippet name, and preserve your last successfully loaded definitions.
To disable native snippet suggestions, set `editor.completion.snippets = false`.
This is independent of `editor.lsp.completion.snippets`.

### Workspace snippets and reloads

Put project definitions in `.mitos/snippets.toml` at the document's workspace
root, using the same format. Mitos loads these when the workspace is trusted.
A project definition replaces the user definition with the same name, including
its prefixes and language scope. Other user definitions remain available.
Each open workspace has its own definitions.

After saving changes, run `:snippets-reload` or `:config-reload`. Reloading reads
user and open workspace definitions together. A parse or read error preserves
the previous definitions and reports the file. Removing a project definition
restores the corresponding user definition after reload. Changes to trusted
workspace settings may require renewing workspace trust.

### Browse, insert, and edit snippets

Bind `snippet_picker` to browse the snippets available in the current language:

```toml
[keys.normal]
A-s = "snippet_picker"
```

The picker shows names, prefixes, and descriptions. Select an entry and press
`Enter` to insert it. To bind a particular snippet directly, use a named command:

```toml
[keys.normal]
A-e = ':snippet-insert "Error check"'
```

Insertion replaces selected text in select mode; larger selections, including
an active placeholder, are replaced in other modes too. A single insert cursor
inserts without deleting the character after it. Use `TM_SELECTED_TEXT` to wrap
a selection. Insertion enters insert mode and selects the first placeholder.
Undo restores the original text as part of the normal insert undo group.

| Command | Action |
| --- | --- |
| `snippet_picker` | Browse and insert available snippets. |
| `:snippet-insert <name>` | Insert a snippet by its exact name. |
| `:snippets-open` | Open the user `snippets.toml`. |
| `:snippets-open-workspace` | Open this document's workspace `snippets.toml`. |
| `:snippets-reload` | Reload snippet definitions. |

A picker choice expires if its document, selection, language, workspace trust,
or definitions change. Reopen the picker to choose from the current context.

### Choice placeholders

`${1|one,two,three|}` starts with `one` selected. Type to replace it, or bind
`next_snippet_choice` and `prev_snippet_choice` to cycle its values:

```toml
[keys.insert]
A-n = "next_snippet_choice"
A-p = "prev_snippet_choice"
```

Linked occurrences of the same placeholder change together. Navigate with the
normal [placeholder commands](#snippet-placeholders); `$0` remains the final stop.
Nested snippets replace the selected placeholder and retain the outer session
when their cursor instances align.

### Clipboard, date, and random variables

`CLIPBOARD` reads the system clipboard when the body references it. Repeated
references share one captured value. No clipboard read occurs for other bodies.

Date and time variables use local time: `CURRENT_YEAR`, `CURRENT_YEAR_SHORT`,
`CURRENT_MONTH`, `CURRENT_MONTH_NAME`, `CURRENT_MONTH_NAME_SHORT`, `CURRENT_DATE`,
`CURRENT_DAY_NAME`, `CURRENT_DAY_NAME_SHORT`, `CURRENT_HOUR`, `CURRENT_MINUTE`,
`CURRENT_SECOND`, and `CURRENT_MILLISECOND`. `CURRENT_SECONDS_UNIX` and
`CURRENT_MILLISECONDS_UNIX` give Unix timestamps; `CURRENT_TIMEZONE_OFFSET`
uses `+HH:MM` or `-HH:MM`, and `CURRENT_TIMEZONE_NAME` uses the IANA name when
available. Month and weekday names are English.

`RANDOM` produces six decimal digits, `RANDOM_HEX` six hexadecimal digits, and
`UUID` a version 4 UUID. Each cursor gets its own values. Repeated references
within a snippet instance share a value. Native completion previews and
acceptance use the same captured variables, even while you browse suggestions.

Bodies use LSP snippet syntax, including placeholders, nested defaults, choices,
and regex transforms. Regexes use Rust syntax. Snippet definitions contain text;
they do not evaluate Lua, Python, shell commands, or other executable expressions.

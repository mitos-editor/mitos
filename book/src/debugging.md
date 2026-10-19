# Debugging (DAP)

Set breakpoints, step through code, and inspect variables with a debug adapter
installed for your language. Debugging support is experimental.

## Configure an adapter

Debug adapters are configured per language in
[`languages.toml`](./languages.md#languagestoml-files). Some languages already
have an adapter and launch templates configured. For example, Rust uses
`lldb-dap` by default; its executable must be available on `PATH`.

To configure or override an adapter, add a language entry to your user
`languages.toml`. This example matches the built-in Rust `binary` template:

```toml
[[language]]
name = "rust"

[language.debugger]
name = "lldb-dap"
transport = "stdio"
command = "lldb-dap"

[[language.debugger.templates]]
name = "binary"
request = "launch"
completion = [{ name = "binary", completion = "filename" }]
args = { program = "{0}" }
```

Use `request = "launch"` to start a program or `"attach"` to connect to a
running one. `completion` defines the prompts; `{0}`, `{1}`, and so on insert
your answers into `args`. The program starts in Mitos's current working
directory unless you set `cwd` in the template's `args`.

See the [debugger configuration reference](./configuration.md#languagedebugger)
for TCP transport, attach templates, and the complete list of fields.
Project-local configuration in `.mitos/languages.toml` is subject to
[workspace trust](./workspace-trust.md).

## Start a session

1. Build the program you want to debug, with debug information enabled.
2. Open a source file in Mitos. Its language determines which adapter and
   templates are used.
3. Press `Space G b` on a line to toggle a breakpoint.
4. Press `Space G l`, choose a template, and fill in its prompts.
5. When execution pauses, step through the code or inspect variables using
   the commands below.

`G` is uppercase. Press the keys in sequence. The debug menu stays active
after a command; while it is active, use the suffix keys such as `n` or `c`
directly. Press `Esc` to leave the menu.

You can also start a template from the command line. For a Rust binary built
with `cargo build`, for example:

```text
:debug-start binary target/debug/my-program
```

Replace `my-program` with your executable's name. To connect to an already
running TCP debug adapter, use
`:debug-remote <ip>:<port> <template> [parameters...]`, for example
`:debug-remote 127.0.0.1:4711 binary target/debug/my-program`.

## Debugging commands

These bindings are available in normal and select mode:

| Keys | Action |
| --- | --- |
| `Space G l` | Select a template and start debugging |
| `Space G b` | Toggle breakpoints on selected lines |
| `Space G c` | Continue execution |
| `Space G h` | Pause execution |
| `Space G n` | Step over |
| `Space G i` | Step in |
| `Space G o` | Step out |
| `Space G v` | Browse variables |
| `Space G s s` | Switch session |
| `Space G s t` | Switch thread |
| `Space G s f` | Switch stack frame |
| `Space G r` | Restart the session |
| `Space G t` | End the session |
| `Space G Ctrl-c` | Edit the breakpoint condition on the current line |
| `Space G Ctrl-l` | Edit the breakpoint log message on the current line |
| `Space G e` / `Space G E` | Enable / disable exception breakpoints |

Use `:debug-eval <expression>` to evaluate an expression in the selected frame.
Expandable results open in the variable browser. Run the command again for a new
result. Supported expressions depend on the debug adapter.

## Inspecting variables

While paused, use `Space G v` to browse the selected frame's variables. Select a
scope, such as locals or globals, and press `Enter` to see its values.
Entries marked `▸` have children; press `Enter` to open them.

Type to filter the list. Select `Load more…` to see more entries in large objects
or arrays. Select `.. (back)` to return to the parent. Your filter and selection
are kept when you return or when values refresh.

Press `Esc` to close the browser. After resuming execution or switching the
session, thread, or frame, use `Space G v` again to inspect the current values.

## Inline debug values

While execution is paused, Mitos can show values such as `answer = 42` beside
the code. The values follow the selected frame and disappear when execution
resumes. Available values depend on your language and debug adapter.

Long values are shortened to fit. Inline values are hidden while the file has
unsaved changes. Use `Space G v` to browse variables and expand objects.

Inline values are on by default. To turn them off in `config.toml`:

```toml
[editor.debugger]
inline-values = false
```

Use `:toggle debugger.inline-values` to show or hide them for the current
editor session.

## Stack frames

Use `Space G s f` to choose a frame in the paused thread. Selecting a frame
opens its source and updates inline values. Use `Space G v` or `:debug-eval`
to inspect values in that frame. Select `Load more…` to see more frames in a
deep call stack.

## Multiple sessions

You can debug several programs at once. Each launch selects the new session;
use `Space G s s` to switch between them. The picker marks the selected session
with `*`, and the statusline shows its name, state, and the total session count.
Child sessions started by an adapter appear in the picker too.

Stepping, continuing, inspecting variables, and ending a session apply to the
selected session. Use `Space G t` to end it or cancel a launch that is still
starting.

File breakpoints are shared across sessions. Save file edits to update
breakpoint locations used by the debugger.

## Adapter source documents

Some adapters provide source code that differs from the file on disk, such as
generated code. Mitos opens it in a read-only buffer. Copy text to a new buffer
if you want to edit or save it.

You can set breakpoints, conditions, and log messages in the current adapter
source. These breakpoints last until the source changes or the session ends.
The buffer stays open for reading afterwards. Select a frame again to open
updated source.

## Terminal for the debugged program

Some adapters ask Mitos to launch the program in another terminal or pane.
Mitos detects supported tmux, Zellij, WezTerm, Kitty, and Alacritty sessions,
and uses Windows Terminal or Conhost on Windows when needed. See
[launcher configuration](./configuration.md#terminallauncher) for detection details.

If your terminal is not detected, configure a command that accepts the program
and its arguments. For example, to use a new Kitty window:

```toml
[terminal.launcher]
command = "kitty"
args = ["--"]
```

## Troubleshooting

- **No debug adapter available for language:** check the current file's
  language and its `[language.debugger]` configuration.
- **Failed to start debug client:** check the adapter's executable path,
  arguments, and transport.
- **No debug config with given name:** use a template name defined in
  `[[language.debugger.templates]]`.
- **Workspace is not trusted:** use `:workspace-trust` to allow debugging in
  a workspace you trust. See [workspace trust](./workspace-trust.md).

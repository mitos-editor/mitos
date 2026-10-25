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
3. Press `Space G` to enter debug mode, then `b` on a line to toggle a breakpoint.
4. Press `L`, choose a template, and fill in its prompts.
5. When execution pauses, step through the code or inspect variables using
   the commands below.

`G` and `L` are uppercase. The statusline shows `DBG` while debug mode is active.
Debug commands use the single keys below, without a persistent help popup.
Use `h`, `j`, `k`, and `l` to navigate normally. Debug mode stays active when
opening source files or changing splits, including when selecting stack frames.

Press `?` to open searchable debug command help with your current bindings.
Choose a command and press `Enter` to run it, or `Esc` to close help. Press `Esc`
while in DBG to return to normal mode. Entering or leaving debug mode does not
start or stop a session. Editing commands such as `a` temporarily enter insert
mode. Press `Esc` to finish editing and return to DBG; press `Esc` again to
return to NOR. Leaving select mode also restores DBG when entered from it.
A session ending while you type leaves you in insert mode; Escape still returns
to DBG, where you can launch another session.
See [key remapping](./remapping.md) to customize `[keys.debug]`.

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

Press `Space G` in normal or select mode, then use these bindings in debug mode:

| Keys | Action |
| --- | --- |
| `L` | Select a template and start debugging |
| `b` | Toggle breakpoints on selected lines |
| `c` | Continue execution |
| `H` | Pause execution |
| `n` | Step over |
| `i` | Step in |
| `o` | Step out |
| `v` | Browse variables |
| `s s` | Switch session |
| `s t` | Switch thread |
| `s f` | Switch stack frame |
| `r` | Restart the session |
| `t` | End the session |
| `Ctrl-c` | Edit the breakpoint condition on the current line |
| `Ctrl-l` | Edit the breakpoint log message on the current line |
| `e` / `E` | Enable / disable exception breakpoints |
| `?` | Show debug commands and current bindings |
| `Esc` | Return to normal mode |

Launch and pause use uppercase `L` and `H` to keep `h` and `l` available for
navigation. If you used the previous debug menu, replace `Space G l` with
`Space G L` and `Space G h` with `Space G H`.

Use `:debug-eval <expression>` to evaluate an expression in the selected frame.
Expandable results open in the variable browser. Run the command again for a new
result. Supported expressions depend on the debug adapter.

## Execution line

While paused, the selected stack frame's source line is highlighted in every
split displaying that source. A `▶` in the gutter marks its first visual row;
soft-wrapped continuations keep the line highlight. Moving the editing cursor
does not move the execution highlight.

The highlight follows the selected session, thread, and stack frame, and clears
when execution resumes or the session ends. It works in all editor modes.
Unsaved edits hide both the highlight and arrow because the adapter's line
numbers may no longer match. Saving or undoing the edits restores them.

Themes can customize the line with `ui.highlight.frameline` and the arrow with
`ui.debug.active`. The line falls back to `ui.highlight` when no frame-line
style is defined. See [themes](./themes.md) for customization.

## Inspecting variables

While paused, use `v` in debug mode to browse the selected frame's variables.
Select a scope, such as locals or globals, and press `Enter` to see its values.
Entries marked `▸` have children; press `Enter` to open them.

Type to filter the list. Select `Load more…` to see more entries in large objects
or arrays. Select `.. (back)` to return to the parent. Your filter and selection
are kept when you return or when values refresh.

Press `Esc` to close the browser. After resuming execution or switching the
session, thread, or frame, use `v` in debug mode again to inspect the current values.

## Inline debug values

While execution is paused, Mitos can show values such as `answer = 42` beside
the code. The values follow the selected frame and disappear when execution
resumes. Available values depend on your language and debug adapter.

Long values are shortened to fit. Inline values are hidden while the file has
unsaved changes. Use `v` in debug mode to browse variables and expand objects.

Inline values are on by default. To turn them off in `config.toml`:

```toml
[editor.debugger]
inline-values = false
```

Use `:toggle debugger.inline-values` to show or hide them for the current
editor session.

## Stack frames

Use `s f` in debug mode to choose a frame in the paused thread. Selecting a frame
opens its source and updates inline values. Use `v` in debug mode or `:debug-eval`
to inspect values in that frame. Select `Load more…` to see more frames in a
deep call stack.

## Multiple sessions

You can debug several programs at once. Each launch selects the new session;
use `s s` in debug mode to switch between them. The picker marks the selected
session with `*`, and the statusline shows its name, state, and total session count.
Child sessions started by an adapter appear in the picker too.

Stepping, continuing, inspecting variables, and ending a session apply to the
selected session. Use `t` in debug mode to end it or cancel a launch that is still
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

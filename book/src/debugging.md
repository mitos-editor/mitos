# Debugging (DAP)

Mitos has experimental support for the Debug Adapter Protocol (DAP) for setting breakpoints,
stepping through code, and inspecting variables. Install a debug adapter
separately and configure it for the language you want to debug.

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

Each template defines a `launch` or `attach` request and adapter-specific
arguments. `completion` defines the input prompts; `{0}`, `{1}`, and so on
are replaced with the supplied values. Completion defaults apply to omitted or
empty values, including command-line launches. Missing parameters are reported
before the adapter starts. A whole placeholder containing an unsigned integer
becomes a JSON number; embedded substitutions and literal strings remain strings.
Mitos uses the editor's current working directory for `cwd` unless the template
sets it explicitly.

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

## Multiple sessions

You can run several debugging sessions at once, including targets started with
`Space G l`, `:debug-start`, or `:debug-remote`. Each launch selects its new session
immediately. Adapter-created child sessions stay in the background. Child
sessions work with both stdio and TCP adapters; stdio children use their own
adapter process.

Use `Space G s s` to select a session. The picker shows its number, name, state,
and parent, with `*` marking the selected session. Selecting a stopped session
opens its current stack frame. The `debug-session` statusline element shows the
selected session and total session count; it is included in the default layout.

Continue, pause, stepping, restart, variables, evaluation, and termination apply
to the selected session. Ending a session restores the previous selection.
`Space G t` also cancels a selected session that is still starting. Source
breakpoints are shared across sessions. Markers follow edits and undo/redo locally;
Mitos sends updated locations after saving or reloading the source. Renaming a
source preserves identities and clears the old adapter locations. Discarding
unsaved changes restores markers to the saved source. Background sessions keep their own
execution state and do not change the selected session when they initialize or stop.

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

Use `:debug-eval <expression>` to evaluate an expression in the current debug
context. Supported expressions and debugging capabilities depend on the adapter.

## Inspecting variables

While stopped, use `Space G v` to browse the current frame's scopes. Select a
scope and press Enter to load its values. Entries marked `▸` have children;
press Enter to open them. Expensive scopes are labeled and load only when opened.

Large containers load 100 values at a time. Select `Load more…` to append the next
page while keeping the current filter and selection.

Select `.. (back)` to return to the parent, keeping its filter and selection.
Escape closes the browser. Resuming execution or switching the session, thread,
or frame invalidates the displayed values. Reopen the browser to refresh them.

Adapter invalidation and expression evaluation refresh values in place, keeping
the open object, filters, and selection. If an object disappears, the browser
returns to its nearest surviving parent.

## Stack frames

Use `Space G s f` to select a frame in the stopped thread. With adapters that
support delayed stack loading, Mitos loads 32 frames at a time. Select `Load more…`
to append frames without losing the picker filter or selection. Selecting a frame
opens its source and makes that frame the context for variables and evaluation.

## Adapter source documents

If a frame's source is provided by the adapter, Mitos opens it in a named,
read-only document. The adapter's content takes precedence over a local file
with the same name. These documents cannot be edited, saved, or renamed; copy
text to a new buffer to work with it. Open source documents remain available as
snapshots after the debugging session ends. If the adapter changes or removes a
source, its old document becomes a historical snapshot too. Select a frame again
to open the current source; historical snapshots no longer accept debugger source
operations.

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

The launcher is needed only when the adapter requests a terminal; it is
separate from the debug adapter's own executable and launch template.

## Troubleshooting

- **No debug adapter available for language:** check the current file's
  language and its `[language.debugger]` configuration.
- **Failed to start debug client:** check the adapter's executable path,
  arguments, and transport.
- **No debug config with given name:** use a template name defined in
  `[[language.debugger.templates]]`.
- **Workspace is not trusted:** use `:workspace-trust` to allow debugging in
  a workspace you trust. See [workspace trust](./workspace-trust.md).

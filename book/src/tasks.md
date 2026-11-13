# Build and test tasks

Tasks run named shell commands while you keep editing. Output streams into a
read-only buffer that supports normal navigation, selection, copying, and search.
Build and test failures can populate the [quicklist](./quicklist.md), so you can
jump to a failure, fix it, and rerun the same task.

## Configure tasks

Add tasks to your user `config.toml` or a project's `.mitos/config.toml`:

```toml
[editor.tasks.build]
command = "cargo build --message-format short --color never"

[editor.tasks.test]
command = "cargo test --message-format short --color never"
env = { RUST_BACKTRACE = "1" }
```

Project configuration is loaded only under the
[workspace trust policy](./workspace-trust.md). Review a project's commands
before trusting its `.mitos/` configuration. Defining a task does not run it.
Running or rerunning a task requires a trusted workspace.

Each task accepts these fields:

| Field | Meaning |
| --- | --- |
| `command` | Required command string, passed to the configured editor shell. |
| `cwd` | Optional working directory. Relative paths start at the workspace root; the default is the workspace root. |
| `env` | Optional table of variables added to or overriding the inherited environment. |
| `error-pattern` | Optional regular expression for extracting file locations. Omit it to use the built-in parser. |

Tasks use the [editor shell](./shell.md#choose-a-shell), including its quoting
rules. They inherit the editor process's environment, with `env` applied on top.
Standard input is closed: commands that need a terminal or interactive input
should run in your terminal.

Tasks do not save buffers. Save your changes with `:write-all` before running a
build or test if it should use your latest edits.

## Run, inspect, and rerun

1. Run `:task` and select a task, or run it by name with `:task build`.
2. Read the output as it arrives. Use `/` to search it and ordinary buffer
   navigation to inspect earlier output.
3. After completion, use `]q` and `[q` to visit parsed failures, or `Space q`
   to choose one from the quicklist.
4. Edit and save the affected files, then run `:task-rerun`.

| Command | Action |
| --- | --- |
| `:task [name]` | Run a named task, or choose one from a picker. |
| `:task-rerun` | Run the last task again using its captured command, directory, and environment overrides. |
| `:task-cancel` | Stop the running task. |
| `:task-output` | Reopen the latest task's retained output. |
| `:task-errors` | Replace the quicklist with locations parsed from the current task's output. |

Only one task runs at a time. Cancel it or wait for completion before starting
another. The output records the exit status or cancellation, and remains
available after the task finishes. Rerunning uses the previous invocation even
if you have since changed the task's configuration.

Task completion replaces the quicklist with the task's parsed locations. Use
`:task-errors` to refresh it while a task runs or restore it after another picker
has replaced it. A task with no parsed locations clears the quicklist.

Each output transcript retains its most recent 1 MiB of UTF-8 text; older text
is discarded. At most 10,000 distinct failure locations are retained per run.
Task output is available for the current editor session; starting another task
replaces the previous task output.

## Parse failure locations

The built-in parser recognizes output in these forms:

```text
src/main.rs:12:5: error: expected expression
handler_test.go:42: expected status 200, got 500
```

Lines and columns are one-based. Relative file paths are resolved from the
task's working directory. Lines without a location remain in the output buffer
and do not create quicklist entries. This includes test failures that report
only a test name without a source location.

For another format, supply `error-pattern`. Use named `path` and `line` captures;
`column` and `message` are optional. Omit `error-pattern` for the default parser.
An invalid pattern or one missing the required captures prevents the task from
starting.

For example, TypeScript's plain output uses `file(line,column): message`:

```toml
[editor.tasks.typecheck]
command = "npx tsc --noEmit --pretty false"
error-pattern = '^(?P<path>.+)\((?P<line>\d+),(?P<column>\d+)\):\s*(?P<message>.*)$'
```

Use TOML single-quoted strings for regular expressions so backslashes stay
literal. Configure tools to emit plain output when possible.

For a Go service in a repository subdirectory:

```toml
[editor.tasks.go-test]
command = "go test ./..."
cwd = "services/api"
env = { CGO_ENABLED = "0" }
```

## Debugger output

Use `:debug-output` to open the retained output for the selected debugger session,
or the most recently ended session when none is running. Pass a session number,
such as `:debug-output 2`, to inspect another retained session.
It uses a read-only buffer with the same navigation and search commands as task
output. Standard output, standard error, and console messages keep their category
labels. Telemetry messages are excluded.

Mitos retains all live sessions and the eight most recently ended sessions,
with a 1 MiB limit per transcript. Closing an output buffer keeps the retained
text available to reopen. Transcripts are discarded when you quit Mitos.

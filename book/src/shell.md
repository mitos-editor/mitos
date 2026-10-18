# Shell commands

Mitos can send selections to a shell command and use its output in your
document. Each selection is processed separately.

| Key | Action |
| --- | --- |
| `\|` | Send each selection to a command and replace it with output |
| `Alt-\|` | Send each selection to a command without changing the document |
| `!` | Insert command output before each selection |
| `Alt-!` | Insert command output after each selection |
| `$` | Keep selections for which the command exits successfully |

Press the key, type the command in the prompt, and press `Enter`. Undo text
changes with `u`.

## Examples

These examples use Unix shell tools:

- Select text, press `|`, and enter `tr '[:lower:]' '[:upper:]'` to uppercase it.
- Select several lines, press `|`, and enter `sort` to sort them together.
  If you have one selection per line, each line is processed separately.
- Press `!` and enter `date` to insert the current date and time.
- With one selection per line, press `$` and enter `grep -q TODO` to keep only
  lines containing `TODO`.

## Run commands without inserting output

Use `:sh <command>` to run a shell command, for example `:sh make`.
The command runs in Mitos's current working directory. Use `:pwd` to inspect
that directory and `:cd <path>` to change it.

The equivalent command-mode operations are `:pipe`, `:pipe-to`,
`:insert-output`, and `:append-output`. These support
[expansions](./command-line.md#expansions), such as `%{buffer_name}`.
Double literal percent signs in these commands: `:insert-output date +%%F`.

## Choose a shell

The default shell is `sh -c` on Unix and `cmd /C` on Windows. Commands must use
the syntax and tools available in that shell. To choose another shell, set
its command prefix in `config.toml`, for example:

```toml
[editor]
shell = ["bash", "-c"]
```

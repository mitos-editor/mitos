---
title: Troubleshooting
description: Diagnose runtime, language-server, configuration, and terminal issues.
---

Start with Mitos' built-in health check:

```sh
ms --health
```

It reports the runtime paths Mitos searches, clipboard support, configured
languages, and tools it could not find. For one language, run
`ms --health <language>`; for example, `ms --health rust`.

## Language features are missing

Language servers are separate programs and must be installed on your system.
Check the configured server and whether its executable is available with:

```sh
ms --health <language>
```

After installing or reconfiguring a server, use `:lsp-restart`. See
[Language servers](../lsp/) and the generated [language support](../lang-support/)
table for the server Mitos expects.

If the workspace shows the restricted-mode indicator, local
`.mitos/languages.toml` settings may be blocked. Review the files and run
`:workspace-trust` if you want Mitos to load them. See [Workspace trust](../workspace-trust/)
for the security model.

## Themes or syntax files are missing

The `ms` binary depends on runtime files for themes, queries, and language
configuration. Run `ms --health` to see the searched locations. When using a
custom build or unpacking a release manually, point Mitos at the runtime:

```sh
export MITOS_RUNTIME=/path/to/mitos/runtime
```

The [installation](../install/) and [building from source](../building-from-source/)
pages describe the normal runtime layout.

## Configuration does not load

Open the active user configuration with `:config-open`, then reload it with
`:config-reload`. Syntax errors are shown when Mitos starts or reloads the file.

Project configuration belongs in `.mitos/config.toml` or
`.mitos/languages.toml` and is subject to [workspace trust](../workspace-trust/).
You can also test a specific file independently with `ms --config <file>`.

## A key binding does not work

Check the [default keymap](../keymap/) and your remappings first. Terminal
emulators and multiplexers can intercept combinations before Mitos receives
them, so test the same binding in a plain terminal session if possible.

## Collect logs

Run Mitos with `-v`, `-vv`, or `-vvv` for progressively more detailed logging.
Inside the editor, `:log-open` opens the current log file. To choose a separate
path for a reproducible report, start Mitos with `--log <file>`.

When reporting a problem, include the output of `ms --version`, the relevant
`ms --health` check, your operating system and terminal, and the smallest steps
that reproduce it.

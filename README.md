<div align="center">

# Mitos

[![Build status](https://github.com/mitos-editor/mitos/actions/workflows/build.yml/badge.svg)](https://github.com/mitos-editor/mitos/actions/workflows/build.yml)
[![License](https://img.shields.io/github/license/mitos-editor/mitos)](./LICENSE)

</div>

![Mitos editing Rust code with completion suggestions and documentation](./book/src/screenshots/autocomplete.png)

Mitos is a modern, batteries-included, modal text editor written in Rust. It is a fork of [Helix](https://github.com/helix-editor/helix) and continues its selection-first editing model with multi-select, built-in language server support, and tree-sitter-powered syntax awareness.

## Why Fork Helix?

Helix built a strong foundation. Mitos is a chance to build on it at a faster pace, experiment with new features and different approaches to the architecture, and see where it can take us. For now, expect less stability and occasional breaking changes. Nevertheless, the end goal is similar - fast, robust, and extensible editor for all your needs.

## Features

- Selection-first modal editing
- Multiple selections
- Built-in language server and debug adapter support
- Incremental syntax highlighting and code editing via tree-sitter
- A terminal UI built with Ratatui

## Installation

On Linux and macOS:

```sh
curl -fsSL https://mitos.computer/install.sh | sh
```

On Windows, run in PowerShell:

```powershell
& ([scriptblock]::Create((Invoke-RestMethod https://mitos.computer/install.ps1)))
```

The installers select your platform, verify the download, and install the executable with its runtime for your user account.

Download a binary archive for Linux, macOS, or Windows from [GitHub Releases](https://github.com/mitos-editor/mitos/releases/latest). Extract it and keep `runtime/` beside `ms` (`ms.exe` on Windows), then add the extracted directory to your `PATH`.

See the [installation guide](https://mitos.computer/docs/install.html) for platform instructions.

### Building from Source

Mitos requires Rust 1.97.1 or newer.

```sh
git clone https://github.com/mitos-editor/mitos
cd mitos
cargo xtask install
ms --health
```

This installs Mitos with its themes and syntax highlighting, normally in `~/.cargo/bin`. Add that directory to your `PATH` if needed.

See [building from source](https://mitos.computer/docs/building-from-source.html) for custom install locations and development builds.

## AI Policy

AI-assisted code is welcome. You are responsible for understanding, reviewing, and testing what you submit. Write discussions and opinions in your own voice. See the [AI policy](./AI_POLICY.md) for details.

---

Mitos is a fork of [Helix](https://github.com/helix-editor/helix). The fork was created from commit `f9928f57f` and retains the complete upstream Git history.

The covered source files remain licensed under the Mozilla Public License 2.0 (`MPL-2.0`). The unmodified license text is distributed in [`LICENSE`](./LICENSE), and Cargo package metadata continues to declare `MPL-2.0`.

**Mitos is not endorsed by or affiliated with the Helix project or its maintainers.**

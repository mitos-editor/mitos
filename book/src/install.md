# Installation

Mitos runs on Linux, macOS, and Windows. The executable is called `ms`.
Use the [installer](#installer), download a [release archive](#release-archives),
or install the [Debian package](#debian-and-ubuntu).
You can also [build from source](./building-from-source.md).

## Installer

### Linux and macOS

```sh
curl -fsSL https://mitos.computer/install.sh | sh
```

The installer selects the right archive, verifies its SHA-256 checksum, and
keeps the executable and runtime together in `~/.local/share/mitos/`.
It links `ms` into `~/.local/bin`, without sudo or changes to your shell files.
If that directory is missing from your `PATH`, it prints the command to add it.

To install a specific version:

```sh
curl -fsSL https://mitos.computer/install.sh | sh -s -- --version 0.1.0
```

Rerun the installer to upgrade. Previous versions stay installed, so running
it with an older `--version` switches back. Use `--install-dir` and `--bin-dir`
to choose other directories. The installer leaves an existing `ms` from another
installation alone. View the [shell installer source](https://github.com/mitos-editor/mitos/blob/main/book/install/install.sh).

### Windows

Run in PowerShell:

```powershell
& ([scriptblock]::Create((Invoke-RestMethod https://mitos.computer/install.ps1)))
```

To pin a version, add `-Version 0.1.0`:

```powershell
& ([scriptblock]::Create((Invoke-RestMethod https://mitos.computer/install.ps1))) -Version 0.1.0
```

The installer verifies the ZIP's checksum, keeps each version and its runtime
under `%LOCALAPPDATA%\Mitos\versions`, and creates an `ms.cmd` launcher in
`%LOCALAPPDATA%\Mitos\bin`. It adds that directory to your user `Path` and the
current PowerShell session. No administrator access is needed.
Use `-InstallDir` for another location, or `-NoModifyPath` to manage `Path`
yourself. Rerun to upgrade or select another version.
View the [PowerShell installer source](https://github.com/mitos-editor/mitos/blob/main/book/install/install.ps1).

### Uninstall

On Linux/macOS, remove the installer-created `~/.local/bin/ms` symlink and
`~/.local/share/mitos` directory. On Windows, remove `%LOCALAPPDATA%\Mitos` and
its `bin` entry from your user `Path`. Adjust these paths for custom installs.
Your configuration and personal dictionaries remain in the Mitos configuration
and state directories.

## Release archives

Download the archive for your system from
[GitHub Releases](https://github.com/mitos-editor/mitos/releases/latest):

| System | Intel / AMD (x86_64) | ARM64 (aarch64) |
| --- | --- | --- |
| Linux | `mitos-vVERSION-x86_64-linux.tar.xz` | `mitos-vVERSION-aarch64-linux.tar.xz` |
| macOS | `mitos-vVERSION-x86_64-macos.tar.xz` | `mitos-vVERSION-aarch64-macos.tar.xz` |
| Windows | `mitos-vVERSION-x86_64-windows.zip` | `mitos-vVERSION-aarch64-windows.zip` |

`VERSION` is the release number, such as `0.1.0`. Apple Silicon Macs use
`aarch64-macos`; Intel Macs use `x86_64-macos`. Linux archives use glibc.

Extract the archive and keep `runtime/` beside `ms` (`ms.exe` on Windows).
It contains the themes, language queries, dictionaries, and compiled grammars.
No Rust toolchain or grammar compilation is needed.

### Linux and macOS

For example, install version `0.1.0` on an Apple Silicon Mac, using
[GitHub CLI](https://cli.github.com/):

```sh
gh release download v0.1.0 --repo mitos-editor/mitos \
  --pattern 'mitos-v0.1.0-aarch64-macos.tar.xz'
mkdir -p "$HOME/.local/share/mitos" "$HOME/.local/bin"
tar -xJf mitos-v0.1.0-aarch64-macos.tar.xz -C "$HOME/.local/share/mitos"
ln -sfn "$HOME/.local/share/mitos/mitos-v0.1.0-aarch64-macos/ms" \
  "$HOME/.local/bin/ms"
```

For another platform or release, replace the archive and directory names
using the table above. You can also download the archive in your browser.
Add `~/.local/bin` to your shell's `PATH` if it is not already there.
Mitos follows the executable's symlink to find the bundled runtime.

### Windows

Extract the ZIP to a permanent directory, such as
`$HOME\Apps\Mitos\mitos-v0.1.0-x86_64-windows`. Add that directory to your
user `Path` through **Environment Variables**, then open a new terminal.
Keep `ms.exe` and `runtime/` together.

## Debian and Ubuntu

On x86_64 Debian or Ubuntu, download the `.deb` asset from
[GitHub Releases](https://github.com/mitos-editor/mitos/releases/latest), then
install it from the download directory:

```sh
sudo apt install ./mitos_0.1.0-1_amd64.deb
```

Use the downloaded package's filename if it differs. The package includes the
runtime, shell completions, and desktop entry. ARM64 users can use the release
archive or the installer. If your system's glibc is too old, see
[building the Debian package](./building-from-source.md#building-the-debian-package).

## Verify the installation

```sh
ms --version
ms --health
```

If the runtime is missing, keep it beside the executable or set `MITOS_RUNTIME`
to its full path. See [runtime configuration](./building-from-source.md#configuring-mitoss-runtime-files)
for the complete search order. Older runtime files in your user configuration
can override those bundled with a release.

Language servers are installed separately. See [language-server setup](./lsp.md)
to enable completion, diagnostics, and other language features.

# Building from source

Requirements:

Clone the Mitos GitHub repository into a directory of your choice. The
examples in this documentation assume installation into either `~/src/` on
Linux and macOS, or `%userprofile%\src\` on Windows.

- The [Rust toolchain](https://www.rust-lang.org/tools/install), version 1.97.1 or newer
- The [Git version control system](https://git-scm.com/)
- A C++14 compatible compiler to build the tree-sitter grammars, for example GCC or Clang

If you are using the `musl-libc` standard library instead of `glibc` the following environment variable must be set during the build to ensure tree-sitter grammars can be loaded correctly:

```sh
export RUSTFLAGS="-C target-feature=-crt-static"
```

1. Clone the repository:

   ```sh
   git clone https://github.com/mitos-editor/mitos
   cd mitos
   ```

2. Compile from source:

   ```sh
   # Optimized release build
   cargo build --release --locked
   ```
   ```sh
   # Optimized for the current CPU
   cargo install \
      --profile opt \
      --config 'build.rustflags=["-C", "target-cpu=native"]' \
      --path crates/term \
      --locked
   ```

   The release build creates `target/release/ms`; the install command places
   `ms` in Cargo's binary directory. Both commands construct the tree-sitter
   grammars in the local `runtime` folder. `ms` is short for Mitos.

To skip downloading and compiling grammars during the build, set
`MITOS_DISABLE_AUTO_GRAMMAR_BUILD=1`.

> 💡 Tree-sitter grammars can be fetched and compiled if not pre-packaged. Fetch
> grammars with `ms --grammar fetch` and compile them with
> `ms --grammar build`. Installed commands write them to
> the `runtime` directory within the user's Mitos data directory (more
> [details below](#multiple-runtime-directories)).

> 💡 If you only want to build _some_ grammars, see [`use-grammars`](./languages.md#choosing-grammars)

### Configuring Mitos's runtime files

#### Linux and macOS

The **runtime** directory is one below the Mitos source, so either export a
`MITOS_RUNTIME` environment variable to point to that directory and add it to
your `~/.bashrc` or equivalent:

```sh
export MITOS_RUNTIME=~/src/mitos/runtime
```

Or, create a symbolic link in the data directory if it does not already exist:

```sh
mitos_data_dir="${XDG_DATA_HOME:-$HOME/.local/share}/mitos"
mkdir -p "$mitos_data_dir"
ln -s "$PWD/runtime" "$mitos_data_dir/runtime"
```

Run this from the repository root. Review an existing runtime directory before
replacing it, since it may contain your own queries or dictionaries.

#### Windows

Either set the `MITOS_RUNTIME` environment variable to point to the runtime files using the Windows setting (search for
`Edit environment variables for your account`) or use the `setx` command in
Cmd:

```sh
setx MITOS_RUNTIME "%userprofile%\src\mitos\runtime"
```

> 💡 `%userprofile%` resolves to your user directory like
> `C:\Users\Your-Name\` for example.

Or, create a symlink in `%appdata%\mitos\` that links to the source code directory:

| Method     | Command                                                                                |
| ---------- | -------------------------------------------------------------------------------------- |
| PowerShell | `New-Item -ItemType Junction -Target "$Env:USERPROFILE\src\mitos\runtime" -Path "$Env:AppData\mitos\runtime"`     |
| Cmd        | `cd %appdata%\mitos` <br/> `mklink /D runtime "%userprofile%\src\mitos\runtime"`       |

> 💡 On Windows, creating a symbolic link may require running PowerShell or
> Cmd as an administrator.

#### Multiple runtime directories

When Mitos finds multiple runtime directories it will search through them for files in the
following order:

1. The workspace's `runtime/` directory when running through Cargo.
2. `runtime/` subdirectory of OS-dependent mitos user config directory.
3. `$MITOS_RUNTIME`
4. `runtime/` subdirectory of the user data directory: `$XDG_DATA_HOME/mitos/runtime`
   (normally `~/.local/share/mitos/runtime`) on Linux and macOS. On Windows,
   the data and config runtime directories are both `%AppData%\mitos\runtime`.
5. Distribution-specific fallback directory (set at build time with
   `MITOS_DEFAULT_RUNTIME`).
6. `runtime/` subdirectory of path to Mitos executable.

This order also sets the priority for selecting which file will be used if multiple runtime
directories have files with the same name.

Use the data runtime directory for installed runtime assets. Keep personal
query or dictionary overrides in the config runtime directory, which takes
priority. Custom themes can also live in the config directory's `themes/` folder.

`ms --grammar fetch` and `ms --grammar build` write grammar sources and compiled
grammars into the user data directory's `runtime/` subdirectory. When run through
Cargo, they write into the current source checkout's `runtime/` instead, as do
automatic grammar builds. `MITOS_RUNTIME` changes where assets are read, not
where these commands write.

#### Migrating an existing config runtime

Existing config runtimes remain supported and take priority over the data
runtime. Mitos does not move or delete them automatically.

On Linux and macOS, you can move installed runtime assets from
`~/.config/mitos/runtime` to `~/.local/share/mitos/runtime`, or change an existing
runtime symlink to live there. Use the corresponding directories if you set
`XDG_CONFIG_HOME` or `XDG_DATA_HOME`. Review both locations first and keep personal
overrides in the config runtime. On Windows, the config and data runtime paths
are the same, so no move is needed.

If an older grammar remains in the config runtime, it will override a newly
built grammar in the data runtime. Move or remove that older installed grammar
when switching to the data runtime. Run `ms --health` to inspect runtime paths
and grammar availability after migrating.

#### Note to packagers

If you are making a package of Mitos for end users, to provide a good out of
the box experience, you should set the `MITOS_DEFAULT_RUNTIME` environment
variable at build time (before invoking `cargo build`) to a directory which
will store the final runtime files after installation. For example, say you want
to package the runtime into `/usr/lib/mitos/runtime`. The rough steps a build
script could follow are:

1. `export MITOS_DEFAULT_RUNTIME=/usr/lib/mitos/runtime`
1. `cargo build --profile opt --locked`
1. `cp -r runtime $BUILD_DIR/usr/lib/mitos/`
1. `cp target/opt/ms $BUILD_DIR/usr/bin/ms`

This way the resulting `ms` binary will always look for its runtime directory in
`/usr/lib/mitos/runtime` for files not provided by the user's config runtime,
`MITOS_RUNTIME`, or data runtime.

### Validating the installation

To make sure everything is set up as expected you should run the Mitos health
check:

```sh
ms --health
```

The command reports runtime, clipboard, language-server, and grammar status.

### Configure the desktop shortcut

If your desktop environment supports the
[XDG desktop menu](https://specifications.freedesktop.org/menu-spec/menu-spec-latest.html)
you can configure Mitos to show up in the application menu by copying the
provided `.desktop` file:

```sh
cp contrib/Mitos.desktop ~/.local/share/applications
```
It is recommended to convert the links in the `.desktop` file to absolute paths to avoid potential problems:

```sh
sed -i -e "s|Exec=ms %F|Exec=$(readlink -f ~/.cargo/bin/ms) %F|g" \
  ~/.local/share/applications/Mitos.desktop
```

To use another terminal than the system default, you can modify the `.desktop`
file. For example, to use `kitty`:

```sh
sed -i "s|Exec=ms %F|Exec=kitty ms %F|g" ~/.local/share/applications/Mitos.desktop
sed -i "s|Terminal=true|Terminal=false|g" ~/.local/share/applications/Mitos.desktop
```

### Building the Debian package

If the `.deb` file provided on the release page uses a `libc` version higher
than that used by your Debian, Ubuntu, or Mint system, you can build the package
from source to match your system's dependencies.

Install `cargo-deb`, the tool used for building the `.deb` file:

```sh
cargo install cargo-deb
```

After cloning and entering the Mitos repository as previously described,
use the following command to build the release binary and package it into a `.deb` file in a single step.

```sh
cargo deb -- --locked
```

The default uses the release profile. To package an already built
`target/release/ms`, run `cargo deb --no-build`.

The package is written to `target/debian/`. It includes the executable, runtime,
shell completions, desktop entry, and launcher.

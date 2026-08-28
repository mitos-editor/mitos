---
title: Package managers
description: Install Mitos using supported operating-system package managers.
---

## Package manager installation

Mitos is a new fork and is not yet published through third-party package
managers. Build it from source until official release artifacts are available.

## Build from source

```sh
git clone https://github.com/mitos-editor/mitos
cd mitos
cargo build --release
install -Dm755 target/release/ms "$HOME/.local/bin/ms"
```

Ensure `$HOME/.local/bin` is in your `PATH`, then verify the installation:

```sh
ms --health
```

Runtime files can be selected explicitly with `MITOS_RUNTIME`. See
[Building from source](../building-from-source/) for runtime and grammar
details.

## Debian package

Maintainers can build the repository's Debian package metadata with:

```sh
cargo install cargo-deb
cargo build --release
cargo deb --no-build
```

The package is written under `target/debian/`.

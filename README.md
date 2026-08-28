<div align="center">

<h1>
  <img alt="Mitos" height="128" src="logo.svg">
</h1>

</div>

[![Build status](https://github.com/mitos-editor/mitos/actions/workflows/build.yml/badge.svg)](https://github.com/mitos-editor/mitos/actions)

Mitos is a post-modern, modal text editor written in Rust. It is a fork of [Helix](https://github.com/helix-editor/helix) and continues its selection-first editing model with multiple selections, built-in language server support, and tree-sitter-powered syntax awareness.

## Features

- Selection-first modal editing
- Multiple selections
- Built-in language server and debug adapter support
- Incremental syntax highlighting and code editing via tree-sitter
- A terminal UI built with Ratatui

## Building

Mitos requires Rust 1.97.1 or newer.

```sh
git clone https://github.com/mitos-editor/mitos
cd mitos
cargo build --release
./target/release/ms --health
```

The optimized executable is written to `target/release/ms` (`ms` is short for
Mitos). Install it on your `PATH` with `cargo install --path crates/term --locked`.

The Blume documentation site lives in [`website/`](./website). Run it locally
with `npm install` followed by `npm run dev` from that directory. Contributor
guidance is available in [`docs/CONTRIBUTING.md`](./docs/CONTRIBUTING.md).

---

Mitos is a fork of [Helix](https://github.com/helix-editor/helix). The fork was created from commit `f9928f57f` and retains the complete upstream Git history so that original authorship and contribution records remain available.

The covered source files remain licensed under the Mozilla Public License 2.0 (`MPL-2.0`). The unmodified license text is distributed in [`LICENSE`](./LICENSE), and Cargo package metadata continues to declare `MPL-2.0`.

Mitos is not endorsed by or affiliated with the Helix project or its maintainers.

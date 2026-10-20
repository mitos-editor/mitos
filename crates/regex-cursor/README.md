# `regex-cursor` in Mitos

Owns cursor-based regular expression search over discontiguous text, including
Ropey slices. Mitos and the crates.io tree-house dependencies share this local
implementation through the root `[patch.crates-io]` entry.

Imported from [`regex-cursor` 0.1.5](https://crates.io/crates/regex-cursor) at
[`fd4c51c73587981219daf9a36184aabbddae78ee`](https://github.com/helix-editor/regex-cursor/commit/fd4c51c73587981219daf9a36184aabbddae78ee),
originally authored by **Pascal Kuthe**, with engine code adapted from
[`rust-lang/regex`](https://github.com/rust-lang/regex).

The imported code remains licensed under **MIT OR Apache-2.0**. Original license
texts and copyright notices are preserved in [`LICENSE-MIT`](LICENSE-MIT),
[`LICENSE-MIT.regex`](LICENSE-MIT.regex), and [`LICENSE-APACHE`](LICENSE-APACHE).

This import keeps Mitos's existing Ropey 1 API. The Ropey 2 migration by
**Blaž Hrastnik (@archseer)** in
[helix-editor/regex-cursor#6](https://github.com/helix-editor/regex-cursor/pull/6),
adopted in [mitos-editor/regex-cursor#1](https://github.com/mitos-editor/regex-cursor/pull/1),
is separate from this move and can be applied here when Mitos migrates to Ropey 2.

The package keeps version `0.1.5` to satisfy tree-house's dependency requirement;
it is an internal workspace crate and is not published separately.

Run `cargo test -p regex-cursor --locked` from the repository root. The upstream
regex corpus is included in [`testdata`](testdata/README.md).

The upstream README follows.

---

# regex-cursor


This crate provides routines for searching **discontiguous strings** for matches of a [regular expression] (aka "regex"). It is based on [regex-automata] and most of the code is adapted from the various crates in the [regex](https://github.com/rust-lang/regex) repository.

It is intended as a prototype for upstream support for "streaming regex". The cursor based API in this crate is very similar to the API already exposed by `regex`/`regex-automata`. To that end a generic `Cursor` trait is provided that collections can implement.

A sketch of the cursor API is shown below. The string is yielded in multiple byte chunks. Calling advance moves the cursor to the next chunk. Calling backtrack moves the cursor a chunk back. Backtracking is required by this crate. That makes it unsuitable for searching fully unbuffered streams like bytes send over a TCP connection.

``` rust
pub trait Cursor {
    fn chunk(&self) -> &[u8] { .. }
    fn advance(&mut self) -> bool { .. }
    fn bracktrack(&mut self) -> bool { .. }
}
```

Working on this crate showed me that regex backtracks a lot more than expected with most functionality fundamentally requiring backtracking. For network usecases that do not buffer their input the primary usecase would likely be detecting a match (without necessarily requiring the matched byte range). Such usecases can be covered by manually feeding bytes into the hybrid and DFA engines from the regex-automata crate. This approach also has the advantage of allowing the caller to pause the match (async) while waiting for more data allowing the caller to drive the search instead of the engine itself.

The only part of this crate that could be applied to the fully streaming case is the streaming PikeVM implementation. However, there are some limitations:
* only a single search can be run since the PikeVM may look ahead multiple bytes to disambiguate alternative matches
* Prefilters longer than one byte can not work
* utf-8 mode can not be supported (empty matches may occur between unicode boundaries)

Currently, the PikeVM implementation is not written with this use case in mind and may call backtrack unnecessarily, but that could be addressed in the future, but especially the first point is very limiting. The pikevm also does not allow the user to drive the search and would block on network calls for example (no async).

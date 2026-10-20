# Regex test corpus

Copied from [`rust-lang/regex` 1.10.6](https://github.com/rust-lang/regex/tree/ab88aa5c6824ebe7c4b4c72fe5191681783b3a68/testdata)
at commit `ab88aa5c6824ebe7c4b4c72fe5191681783b3a68`. These are the 25 TOML
fixtures loaded by `src/tests.rs`, included here so tests run from a clean Mitos
checkout without a sibling repository or network downloads.

The corpus is licensed under MIT OR Apache-2.0. The original notices are
preserved in [`../LICENSE-MIT.regex`](../LICENSE-MIT.regex) and
[`../LICENSE-APACHE`](../LICENSE-APACHE).

The upstream testdata README follows.

---

This directory contains a large suite of regex tests defined in a TOML format.
They are used to drive tests in `tests/lib.rs`, `regex-automata/tests/lib.rs`
and `regex-lite/tests/lib.rs`.

See the [`regex-test`][regex-test] crate documentation for an explanation of
the format and how it generates tests.

The basic idea here is that we have many different regex engines but generally
one set of tests. We want to be able to run those tests (or most of them) on
every engine. Prior to `regex 1.9`, we used to do this with a hodge podge soup
of macros and a different test executable for each engine. It overall took a
longer time to compile, was harder to maintain and it made the test definitions
themselves less clear.

In `regex 1.9`, when we moved over to `regex-automata`, the situation got a lot
worse because of an increase in the number of engines. So I devised an engine
independent format for testing regex patterns and their semantics.

Note: the naming scheme used in these tests isn't terribly consistent. It would
be great to fix that.

[regex-test]: https://docs.rs/regex-test

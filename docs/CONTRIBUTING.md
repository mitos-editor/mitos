# Contributing

Contributors are very welcome! **No contribution is too small and all contributions are valued.**

Some suggestions to get started:

- You can look at the [good first issue][good-first-issue] label on the issue tracker.
- Help with packaging on various distributions needed!
- To use print debugging to the [Mitos log file][log-file], you must:
  * Print using `log::info!`, `warn!`, or `error!`. (`log::info!("mitos!")`)
  * Pass the appropriate verbosity level option for the desired log level. (`ms -v <file>` for info, more `v`s for higher verbosity)
  * Want to display the logs in a separate file instead of using the `:log-open` command in your compiled Mitos editor? Start your debug version with `cargo run -- --log foo.log` and in a new terminal use `tail -f foo.log`
- Instead of running a release version of Mitos, while developing you may want to run in debug mode with `cargo run` which is way faster to compile
- Looking for even faster compile times? Give [mold](https://github.com/rui314/mold) a try
- If your preferred language is missing, integrating a tree-sitter grammar for
    it and defining syntax highlight queries for it is straightforward and
    doesn't require much knowledge of the internals.
- The pinned toolchain includes rust-analyzer for contributors who do not use
  the Nix development shell.

We provide an [architecture.md][architecture.md] that should give you
a good overview of the internals.

# Auto-generated documentation

Some parts of [the documentation site][docs] are generated from the code itself,
like the list of `:commands` and supported languages. To generate these
files, run

```shell
cargo xtask docgen
```

inside the project. We use [xtask][xtask] as an ad-hoc task runner.

To preview the documentation site, install its dependencies and start Blume:

```shell
cd website
npm install
npm run dev
```

Then visit the local URL printed by Blume.

# Testing

## Unit tests/Documentation tests

Run `cargo test --workspace` to run unit tests and documentation tests in all packages.

## Integration tests

Integration tests for term can be run with `cargo integration-test`. Code
contributors are strongly encouraged to write integration tests for their code.
Existing tests can be used as examples. Helpers can be found in
[helpers.rs][helpers.rs]. The log level can be set with the `MITOS_LOG_LEVEL`
environment variable, e.g. `MITOS_LOG_LEVEL=debug cargo integration-test`.

Contributors using MacOS might encounter `Too many open files (os error 24)`
failures while running integration tests. This can be resolved by increasing
the default value (e.g. to `10240` from `256`) by running `ulimit -n 10240`.

## Minimum Stable Rust Version (MSRV) Policy

Mitos keeps an intentionally low MSRV for the sake of easy building and packaging
downstream. We follow [Firefox's MSRV policy]. Once Firefox's MSRV increases we
may bump ours as well, but be sure to check that popular distributions like Ubuntu
package the new MSRV version. When increasing the MSRV, update these three places:

* the `workspace.package.rust-version` key in `Cargo.toml` in the repository root
* the `env.MSRV` key at the top of `.github/workflows/build.yml`
* the `toolchain.channel` key in `rust-toolchain.toml`

[Firefox's MSRV policy]: https://firefox-source-docs.mozilla.org/writing-rust-code/update-policy.html
[good-first-issue]: https://github.com/mitos-editor/mitos/labels/E-easy
[log-file]: https://github.com/helix-editor/helix/wiki/FAQ#access-the-log-file
[architecture.md]: ./architecture.md
[docs]: ../website
[xtask]: https://github.com/matklad/cargo-xtask
[helpers.rs]: ../crates/term/tests/test/helpers.rs

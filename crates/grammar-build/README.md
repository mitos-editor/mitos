# `grammar-build`

Fetches grammar sources and compiles native Tree-sitter libraries from explicit
configuration, runtime output paths, and optional cross-compilation targets.
Callers resolve configuration and workspace trust through `loader` first.
Compiled grammar loading remains in `loader`; this crate owns Git and compiler
invocation, source caching, and rebuild detection. The `grammar-fetch` binary
prepares sources for release CI without building the editor.

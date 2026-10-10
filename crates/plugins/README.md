# plugins

Mitos's capability-checked WebAssembly component runtime and command registry.
The command/lifecycle semantics reuse Matthew Paras's Helix plugin proposal
([helix-editor/helix#8675](https://github.com/helix-editor/helix/pull/8675)); execution
uses lean Wasmtime 48.0.5 and the versioned WIT world in `plugin-api`.

Configuration points to `plugin.toml`. A code package declares ABI3, a relative
component module, capabilities, command documentation, and event subscriptions.
Commands are qualified by the configured plugin name. Every editor generation
owns independent serialized actors, with persistent guest state between calls.
An all-disabled or declarative-only configuration needs no guest executor.

The async manager prepares the entire replacement off-thread, including package
files, digest checks, compilation, import linking, and instantiation. Activation
runs no guest; a failed replacement keeps the existing generation available.
Guests run on dedicated bounded workers and receive document/view metadata by
default. Text requires an explicit bounded, version-checked read. Host services
check declared capabilities, user grants, and live ownership; WASI and ambient
filesystem/network/process access are not installed.

Effects stream through bounded resources and remain staged until the exported
handler succeeds. The owning editor validates the complete batch before applying
native transactions. Traps/interrupted Stores disable only that actor until
reload; typed denials and ordinary handler errors leave a healthy Store usable.
Result-byte reservations remain held through the editor's application callback.

See [the execution contract](RUNTIME.md) for budgets, cancellation, compiler and
memory limits, and [the SDK](../plugin-sdk/README.md) for authoring. Normal runtime tests execute checked
Rust SDK, generated Rust, and generated C source-WASM fixtures without silently
requiring or skipping a guest compiler.

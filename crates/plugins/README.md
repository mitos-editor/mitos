# plugins

Mitos's WebAssembly runtime and plugin command registry. Its registry boundary
and lifecycle entry points follow Helix's plugin proposal
([helix-editor/helix#8675](https://github.com/helix-editor/helix/pull/8675)); execution
uses Wasmi and the versioned `plugin-sdk` protocol.

Plugin configuration points to a `plugin.toml` manifest. The manifest declares
`abi-version`, a relative `module` path, command documentation, and subscribed
events. Commands are qualified by the configured plugin name. Each editor owns
its plugin instances, whose memory persists across command and event calls.

Guests have no imports, WASI, filesystem, network, or process access. The host
passes JSON snapshots and receives validated actions. Modules are limited to
16 MiB, manifests to 64 KiB, linear memory to 64 MiB, tables to 4,096 elements,
and each complete invocation to 10 million fuel units. Request and response
messages are limited to 4 MiB, and responses to 256 actions. Compilation uses
Wasmi's strict limits. Allocation, handlers, deallocation, and module start
functions all run with fuel metering.

A guest trap or invalid ABI response disables that instance and releases its
memory. Other plugins continue running; the command documentation remains
available so errors are understandable. Reloading creates fresh instances.
Oversized host snapshots and ordinary errors returned in `Response.error` do
not disable plugins. Lifecycle `init` and `shutdown` events reach all active
instances; other events require a manifest subscription.

See [`plugin-sdk`](../plugin-sdk/README.md) for authoring and ABI documentation.

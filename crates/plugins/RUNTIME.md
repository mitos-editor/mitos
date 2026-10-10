# Component execution contract

The production runtime uses Wasmtime 48.0.5 with the async component model and
Cranelift. It imports the versioned world in `plugin-api/wit/plugin.wit`; it does
not attach WASI, ambient filesystem handles, network sockets, or native symbols.
Guest code is compiled from source WASM. Package-provided native artifacts are
never deserialized.

On macOS the engine uses Wasmtime's supported Unix signal trap handling via
[`macos_use_mach_ports(false)`](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#method.macos_use_mach_ports).
The editor spawns native children and handles process signals; Wasmtime documents
this trap path for fork-capable embeddings. It avoids the Mach helper's abort on
an interrupted receive while retaining guest trap recovery and forwarding native
faults to the previous signal handler.

The owning editor creates a worker pool only when an enabled code package needs
one. Each plugin has one serialized actor and Store. The dedicated executor has
two guest worker threads, one blocking compiler thread, and Tokio event/global
queue intervals of one. Up to 32 bounded calls may remain suspended in host
services, so waiting for a process or read does not occupy the two available
guest CPU workers. An independent ticker yields guest execution every 2 ms;
generation revocation and target cancellation do not require mailbox space.
Epoch scheduling is a responsiveness target, not a real-time guarantee.

Preparation compiles, links, and instantiates off the editor path without
attaching editor services or dispatching Init. Activation moves an already
prepared Store into an actor. The editor switches the replacement set only once
all packages have prepared; a failed preparation leaves the old generation
available. Compilation admission and waiting have deadlines, but an already
running native compilation cannot be interrupted. Source-complexity admission
limits core functions/types, signature fields, operators, locals, sections,
nesting, and branch-table targets before compilation.

| Resource | Limit |
| --- | --- |
| Source component | 16 MiB per package |
| Active/prepared source | 128 MiB per pool, including replacement generations |
| Guest memory | 64 MiB per Store, 256 MiB across a pool |
| Core resources | 8 memories, 8 tables, 16 instances, 4096 aggregate table entries per Store |
| Guest stacks | 1 MiB WASM stack, 2 MiB async stack |
| Native code | 128 MiB active/cache aggregate; measured from self-serialized artifact plus source size |
| Compilation cache | 16 entries, 64 MiB measured weight |
| Requests | 64 outstanding calls/results per actor, 4 MiB queued bytes, 1 MiB per request |
| Command reservation | 8 of the 64 admission slots |
| Execution/preparation | 5 seconds after execution/preparation begins |
| Effects | 256 actions, 4096 entries, 4 MiB retained bytes per invocation |
| Component resources | 64 handles, including builders and jobs |
| Jobs | 16 owned resources per plugin, with additional native-service editor quotas |
| Completed results | 4 MiB per actor, 32 MiB per pool; held through editor application |
| Diagnostics | 4096 bytes after sanitization |

All guest-to-host imported calls accept at most one string. Prompt content,
picker rows, process arguments/input, and file/storage writes use small resource
builders. This avoids copying several aliased 64 MiB guest strings in one import.
The canonical ABI still lifts that one string before Rust performs its size
check; a malicious call can temporarily allocate up to the guest memory limit
on a worker. Rejected strings are released before any awaited service operation.
Host responses and resource tables have separate retention quotas. These limits
are not a hard cap on editor RSS: native compilation, code metadata, allocator
overhead, stacks, and canonical lifting also consume host memory. In-process
compilation is not a crash or hostile-RSS sandbox.

Effects remain private to an invocation until the exported handler succeeds,
every opened group finishes, and no builder rejection occurred. `finish` alone
does not publish effects. The editor then validates the whole batch against its
generation, document versions, view bindings, and selection revisions before
applying native transactions. A trap or interrupted Store discards all staged
effects and disables only that actor until reload. Typed service/capability
failures keep a healthy Store available.

Job creation follows the cancellation-safe `HostServices` contract: work is
registered before spawning, ownership is returned promptly, and long operations
belong to the returned job. Resource destruction and actor shutdown await job
cancellation/cleanup. The actor drops its Store before acknowledging shutdown.
Retained jobs carry a host-only creation document/view target independently of
guest metadata permissions. Closing that target cancels their watchers and native
work. `JobReady` controls identify one retained resource and coalesce readiness
progress; handlers drain output until Pending/Finished and return to free the
Store. Watcher lifetimes follow the actor generation rather than the invocation
that created the job. A rejected readiness callback revokes and cleans its job.
Consumers retain `CompletedResponse` through their owning editor callback and
use `with_response` when applying or rejecting the response; an abandoned
completion/callback releases its reservations.

Normal unit tests run checked Rust and C source-WASM fixtures through the real
component actor. They cover Unicode reads/edits, state, streamed UI models,
resource destruction, stale generations, typed denials, traps, cancellation,
fairness while services suspend, transactional preparation, and per-plugin and
aggregate memory/result quotas. Fixture regeneration is explicit and never
silently skipped by a host test. The separate benchmark workspace records the
macOS ARM64 and native Linux ARM64 measurements, actual editor input/link-size
gates, and the remaining pinned Linux x86-64 CI gate.

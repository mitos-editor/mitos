# Investigating syntax update latency

Syntax initialization can run in the background. Subsequent edits update trees
synchronously before document observers and language-server synchronization.
Measure that path before changing its ownership or timeout policy.

Prepare the runtime grammars explicitly, then run from the repository root:

```sh
MITOS_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo bench --locked -p language --bench syntax_updates
```

`MITOS_BENCH_RUNTIME` selects a runtime directory containing queries and compiled
grammars. The default is this checkout's `runtime`. Missing grammars fail the run;
the benchmark never fetches them. Injection grammar dependencies are required too,
and the Rust/Markdown fixtures assert that the expected injected layers exist. Built-in language configuration and these
explicit resources keep user configuration out of the experiment.

`MITOS_BENCH_ROWS` (default 1000) controls fixture repetitions and
`MITOS_BENCH_SAMPLES` (default 21) controls retained samples after three warmups.
Run several sizes, including 100, 1000, and 10000 rows. CSV output records fixture
bytes, sample count, and p50/p95/p99 nanoseconds. With few samples the upper
percentiles mostly describe the maximum; retain raw runs and repeat processes.

Fixtures cover flat JSON, Rust format-macro injections, Markdown fenced
TypeScript, and TypeScript. Each measures a local whitespace insertion and a
potentially structural quote insertion. Each sample starts from the same text
and syntax snapshot. Snapshot cloning, initial parsing, and destruction are
outside the incremental timing. The first syntax construction is reported
separately as one observation, not as a distribution or a process-cold guarantee.

The root-tree edit and root-parser stages are an independent comparison using
the same grammar, source, and edit. They exclude injection and local queries.
They are **not** a decomposition of the full update: do not subtract their times
to claim a measured injection cost. Full updates also construct edit descriptors,
update multiple layers, and execute queries. Sample order alternates so either
path can run first.

For interactive attribution, start Mitos with `MITOS_LOG_LEVEL=debug`. Records on
the `mitos::syntax_update` target report edit-descriptor generation and the combined
dependency update in microseconds, together with source bytes, edit count, and
success. Clock reads are disabled at normal log levels. Records contain no source
text. Logging itself perturbs timings; use it to locate a problem, then profile
without debug output.

The dependency owns the remaining phase boundaries. Use a sampling profiler to
distinguish tree editing, parsing, `run_injection_query`, and `run_local_query`
inside `tree_house::Syntax::update`. Record platform, revision, profile, runtime
grammar revisions, fixture size, and whether query compilation was warm. Compare
input-to-frame latency separately: this benchmark excludes feature mapping,
rendering, terminal output, and event scheduling.

Prefer a focused parser/query improvement or a documented large-file policy when
that explains the cost. Moving incremental updates to workers requires an
explicit stale-syntax contract for indentation, structural selections, and other
syntax consumers. Lowering the timeout alone can disable syntax on more edits.

# Recording application performance workloads

Use `tests/performance/workloads.py` to compare production Mitos binaries through
a fixed-size POSIX pseudo-terminal. It creates disposable documents, a workspace
and a real Git history, then records terminal frames and resident memory during
editing, picker navigation, sidebar filtering and history browsing. It does not
change preview policy or set pass/fail timing thresholds.

## Run a comparison

Python 3.11 or newer, Git and a POSIX PTY are required. Build the editor with Git
support, using the same Rust toolchain, profile and runtime resources for both
revisions. From the repository root:

```sh
cargo run --locked --package grammar-build --bin grammar-fetch
cargo run --locked --package grammar-build --bin grammar-build
MITOS_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo build --locked --release -p term --bin ms
python3 -m unittest discover -s tests/performance -p 'test_*.py'
python3 tests/performance/workloads.py \
  --binary target/release/ms --runtime runtime \
  --samples 5 --files 10000 --commits 1000 --previews 20 \
  --output /tmp/mitos-performance.json
```

Grammar preparation fetches the revisions in `languages.toml`. The recorder
itself does not fetch dependencies or compile the editor. Use `--profile opt`
instead of `--release`, and pass `target/opt/ms`, to measure the optimized release
profile. Keep profiles consistent across comparisons.

Run separate processes for the baseline and candidate. Repeat runs on an idle
machine, alternating revision order. Keep each JSON artifact alongside the
revision, build command, runtime revision and machine details. The artifact
records the binary SHA-256, reported editor version, platform, runtime path,
terminal dimensions and fixture sizes. A runtime path alone does not identify
its contents; retain its revision and grammar provenance separately.

All size/count arguments must be positive. Defaults are five samples, 10,000
workspace files, 1,000 commits and 20 preview selections. Start with a small smoke
run, then increase workspace and history sizes. Each sample starts fresh editor
processes. Fixture creation and Git history generation happen before measurement.

The **Performance workloads** GitHub Actions workflow runs only through manual
`workflow_dispatch`. It accepts `release` or `opt`, records JSON as an artifact,
and has no timing gate. Shared runners are useful for collecting evidence;
compare repeated observations before treating a difference as a regression.

## Interpret the observations

The recorder sends real keyboard input and reads production terminal output in a
120-column, 40-row PTY. It replies to supported terminal capability probes and
pairs synchronized-output frame markers across arbitrary read boundaries.
Scenario checks must confirm the intended screen state before a workload is
considered ready: an unrelated redraw or an error message cannot stand in for an
opened picker, loaded preview or history result.

`first_frame_ns` measures the first completed terminal frame observed after the
input write. It can be a loading indicator or an acknowledgement, before results
are available. `ready_frame_ns` requires the scenario's expected screen marker
and a completed frame. Readiness is limited to that visible state; it does not
establish that every background worker has finished. A quiet interval
without another frame is an output observation, not proof of task completion.

The raw measurements retain frame counts, output byte counts and timing values.
Summary p50/p95/p99 values use the nearest-rank convention. With five samples,
p95 and p99 describe the largest observation. Preview-navigation repetitions
from the same process share caches and are not independent cold samples; retain
sample identifiers when comparing them.

`rss_kib` is an optional process RSS observation obtained through `ps`. It is
`null` when the process cannot be measured or `ps` is unavailable. RSS includes
allocator and runtime retention. It is neither a cache-size measurement nor a
peak-memory measurement. A memory increase after preview navigation does not by
itself establish a leak.

Idle observations count completed frames and bytes during a bounded interval.
They do not count CPU wakeups, thread activity or background task completion.
Use a profiler or operating-system tools to investigate those costs. Likewise,
a close/escape input and its resulting frame do not measure when a cancelled
worker releases its resources.

A PTY excludes real terminal rendering, compositor presentation, keyboard
hardware latency and display refresh. Frame timestamps reflect bytes observed
by the recorder after the application writes them. The harness also changes
input pacing and adds terminal emulation and measurement overhead. Use it for
repeatable application comparisons, then verify a user-visible change in an
actual terminal.

The recorder isolates Mitos configuration and XDG data/state/cache directories,
removes inherited Git overrides, and disables global/system Git configuration.
The temporary configuration disables language servers, automatic completion,
file watching and reload polling to reduce unrelated work. These results do not
represent a normal interactive session with those features enabled. Each run
uses temporary fixtures and config files, then terminates its editor processes
and removes the temporary directory. Keep failure diagnostics when a scenario
does not reach its expected screen state; do not compare a failed scenario's
partial observations as successful results.

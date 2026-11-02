# Interactive editing measurements

Run a production release binary through its real input reader, event loop,
renderer and terminal backend in a fixed 120 × 40 pseudo-terminal. These local
measurements end when synchronized output reaches the PTY; they exclude actual
terminal-emulator/display latency. They use Python 3 and Unix PTYs.

Build the editor in release mode and install its Rust runtime grammar first.
Use one unchanged fixture for both revisions. Query compilation and initial file
loading finish before the timed key burst. The driver copies the fixture into
an isolated workspace and never saves edits to the original.

```sh
MITOS_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo build --release -p term --bin ms --locked
python3 benchmarks/interactive/drive.py target/release/ms crates/view/src/document.rs \
  --runtime runtime --output /tmp/mitos-typing-before
python3 benchmarks/interactive/drive.py target/release/ms crates/view/src/document.rs \
  --mode motion --runtime runtime --output /tmp/mitos-motion-before
```

Use distinct output directories and repeat sequentially at least three times per
revision. Do not overlap runs with builds or tests. `result.json` contains every
sample, p50/p95 and frame counts; terminal bytes are retained for comparison.
Normal host scheduling remains uncontrolled, so report process variation and
avoid treating these values as CI thresholds.

`--lsp` starts only the included local mock. It advertises document highlights,
records messages in `requests.jsonl` and withholds highlight responses so frame
counts isolate request scheduling. It overrides Rust's server list. Real user
language servers and configuration are not loaded.

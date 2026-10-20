# Ropey migration performance

The `view` crate's `ropey` benchmark exercises the same public text APIs on three
revisions: Ropey 1 at `90295ab7d`, the initial Ropey 2 migration at `68c903914`, and
the performance changes at `d43100164`. The benchmark harness is copied unchanged
into the two baseline worktrees. Production indexing metrics and SIMD settings
are unchanged by the performance commit.

The fixtures contain 16,384 numbered lines of ASCII or mixed Unicode, including
combining marks, emoji, flags, and CRLF. Workloads cover single and counted
grapheme movement, 128 protocol position round trips, formatter initialization,
a 2,048-grapheme viewport, typing at one or 128 cursors, a large paste, regex
search, and 1,024-line diff insertions at the start, middle, and end of a file.
Typing measures transaction construction and application to a rope snapshot.
Terminal painting, syntax parsing, protocol transport, and file I/O require
separate profiling.

Measurements were collected on 2026-10-20 with an Apple M5 Pro, macOS 26.6, and
Rust 1.97.1 (`aarch64-apple-darwin`), using the release benchmark profile. Each
case uses 30 samples, a 200 ms warmup, and a one-second target measurement time.
These are local sample medians, with background load and thermals uncontrolled.
Viewport formatting, typing, and long backward moves were repeated with a
three-second window to investigate timing variation. These measurements are
evidence for the targeted changes rather than CI timing thresholds.
The complete results, including slower workloads, are in
[ropey-benchmarks.csv](ropey-benchmarks.csv).

Unicode workloads from the complete CSV:

| Workload | Ropey 1 | Initial Ropey 2 | Optimized Ropey 2 |
|---|---:|---:|---:|
| Byte-boundary queries (384 queries) | 11.72 µs | 9.60 µs | 2.52 µs |
| Forward grapheme move | 69.60 ns | 45.10 ns | 33.30 ns |
| UTF-16 protocol conversion (128 round trips) | 64.85 µs | 125.15 µs | 80.55 µs |
| Formatter startup | 211.90 ns | 156.80 ns | 121.40 ns |
| Large diff, middle insertion | 1.53 ms | 2.98 ms | 1.64 ms |

The Unicode middle-insertion diff reduces allocation calls from 68,925 to 4,299,
and requested allocation volume from 10.79 MB to 2.52 MB.
The byte-boundary batch drops from 384 allocation calls to zero.

The migration still has tradeoffs: protocol conversion and large pastes remain
slower than Ropey 1 on these inputs, and large diffs remain slightly slower.
Search and full viewport formatting show smaller changes than the isolated
startup and boundary queries. The full CSV keeps those workloads visible.

Allocation probes warm each operation once and count one execution on the calling
thread, excluding fixture construction. Requested bytes include allocation and
reallocation requests; they measure allocation volume, rather than peak or
retained memory. Timing runs leave allocation counting disabled.

The retained changes use native byte slices, line-local protocol conversions,
borrowed contiguous line tokens, direct character-boundary queries, and character
conversion at the endpoints of grapheme traversal. Long lines keep rope metrics.
A `ChunkCursor` candidate added allocations without a consistent speedup on these
fixtures and was discarded. Single and counted moves remain allocation-free.

There is a pre-existing segmentation limitation at some regional-indicator and
ZWJ chunk seams. Upstream fixes
[unicode-segmentation#175](https://github.com/unicode-rs/unicode-segmentation/pull/175)
and [#172](https://github.com/unicode-rs/unicode-segmentation/pull/172) are not in the
released dependency used here. The traversal tests compare against contiguous
Unicode segmentation for the other cases, and against the existing byte-based
traversal for these sequences, preserving behavior without adding a Git patch to
another dependency.

To reproduce, run from this branch's repository root. Create the two baseline
worktrees and copy only the benchmark harness:

```sh
git worktree add --detach /tmp/mitos-ropey1 90295ab7d
git worktree add --detach /tmp/mitos-ropey2 68c903914
for checkout in /tmp/mitos-ropey1 /tmp/mitos-ropey2; do
  cp -R crates/view/benches "$checkout/crates/view/"
  printf '\n[[bench]]\nname = "ropey"\nharness = false\n' >> "$checkout/crates/view/Cargo.toml"
done
export ropey_bench_root=/tmp/mitos-ropey-bench
export CRITERION_HOME="$ropey_bench_root/criterion"
export MITOS_DISABLE_AUTO_GRAMMAR_BUILD=1
```

Use separate Cargo build directories for the worktrees and share only Criterion
results. Sharing build artifacts between same-version worktrees can reuse the
wrong control binary. Run measurements sequentially with local builds stopped:

```sh
CARGO_TARGET_DIR="$ropey_bench_root/build-v1" cargo bench --manifest-path /tmp/mitos-ropey1/Cargo.toml -p view --bench ropey --locked -- \
  --save-baseline ropey1 --sample-size 30 --warm-up-time 0.2 --measurement-time 1
CARGO_TARGET_DIR="$ropey_bench_root/build-v2" cargo bench --manifest-path /tmp/mitos-ropey2/Cargo.toml -p view --bench ropey --locked -- \
  --save-baseline ropey2-migration --sample-size 30 --warm-up-time 0.2 --measurement-time 1
CARGO_TARGET_DIR="$ropey_bench_root/build-optimized" cargo bench -p view --bench ropey --locked -- \
  --save-baseline ropey2-optimized --sample-size 30 --warm-up-time 0.2 --measurement-time 1
```

For the longer confirmation measurements, repeat the three timing commands with
the positional filter `'layout_viewport|/type($|_)|graphemes_prev/4096'` and
`--measurement-time 3`.

Collect allocation counts separately, then regenerate the CSV:

```sh
MITOS_BENCH_ALLOCATIONS=1 CARGO_TARGET_DIR="$ropey_bench_root/build-v1" cargo bench --manifest-path /tmp/mitos-ropey1/Cargo.toml -p view --bench ropey --locked \
  > "$ropey_bench_root/allocations-ropey1.log" 2>&1
MITOS_BENCH_ALLOCATIONS=1 CARGO_TARGET_DIR="$ropey_bench_root/build-v2" cargo bench --manifest-path /tmp/mitos-ropey2/Cargo.toml -p view --bench ropey --locked \
  > "$ropey_bench_root/allocations-ropey2-migration.log" 2>&1
MITOS_BENCH_ALLOCATIONS=1 CARGO_TARGET_DIR="$ropey_bench_root/build-optimized" cargo bench -p view --bench ropey --locked \
  > "$ropey_bench_root/allocations-ropey2-optimized.log" 2>&1
python3 crates/view/benches/support/summarize.py \
  "$CRITERION_HOME" "$ropey_bench_root" > docs/ropey-benchmarks.csv
```

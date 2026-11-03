//! Measures commit/replay separately, including allocations during each operation.
use buffer::{Buffer, HistoryAction};
use editor_core::{Rope, Transaction};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

struct CountAlloc;
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATED.fetch_add(size, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountAlloc = CountAlloc;

fn main() {
    let original = Rope::from("identifier α😀 = value;\n".repeat(200_000));
    for (name, removed) in [
        ("single_char", 1),
        ("small_fraction", original.len_chars() / 20),
        ("half_document", original.len_chars() / 2),
        ("whole_document", original.len_chars()),
    ] {
        let tx = Transaction::change(
            &original,
            [(0, removed, Some("replacement".into()))].into_iter(),
        );
        let mut commits = Vec::new();
        let mut replays = Vec::new();
        let mut commit_bytes = Vec::new();
        let mut replay_bytes = Vec::new();
        for _ in 0..31 {
            let mut buffer = Buffer::new(original.clone());
            buffer.apply(tx.changes()).unwrap();
            ALLOCATED.store(0, Ordering::Relaxed);
            let start = Instant::now();
            black_box(buffer.commit_changes());
            let elapsed = start.elapsed().as_secs_f64() * 1_000_000.0;
            let allocated = ALLOCATED.load(Ordering::Relaxed);
            commits.push(elapsed);
            commit_bytes.push(allocated);
            ALLOCATED.store(0, Ordering::Relaxed);
            let start = Instant::now();
            let mut plan = buffer.prepare_history(HistoryAction::Undo);
            let replay = buffer.apply_history_step(&mut plan).unwrap().unwrap();
            let elapsed = start.elapsed().as_secs_f64() * 1_000_000.0;
            let allocated = ALLOCATED.load(Ordering::Relaxed);
            replays.push(elapsed);
            replay_bytes.push(allocated);
            black_box(replay);
        }
        commits.sort_by(f64::total_cmp);
        replays.sort_by(f64::total_cmp);
        commit_bytes.sort_unstable();
        replay_bytes.sort_unstable();
        println!(
            "{name}: commit_us={:.6}, commit_alloc_bytes={}, undo_us={:.6}, undo_alloc_bytes={}",
            commits[15], commit_bytes[15], replays[15], replay_bytes[15]
        );
    }
}

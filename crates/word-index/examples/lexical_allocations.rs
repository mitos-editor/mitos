//! Allocation requests for one warm lexical-completion call, without timing.
use editor_core::Rope;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
};
use word_index::WordIndex;
struct Counter;
static BYTES: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        CALLS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        BYTES.fetch_add(size, Ordering::Relaxed);
        CALLS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOC: Counter = Counter;
fn main() {
    for unicode in [false, true] {
        let prefixes = if unicode {
            ["parseÉntry", "préfixFile", "路径File", "filter項目"]
        } else {
            ["parseEntry", "prefixFile", "pathFile", "filterObject"]
        };
        let text = (0..100_000)
            .map(|i| {
                format!(
                    "{}{}{:06} ",
                    prefixes[i % 4],
                    if i % 3 == 0 { "Buffer" } else { "Context" },
                    i
                )
            })
            .collect::<String>();
        let mut index = WordIndex::default();
        index.add_document(&Rope::from_str(&text), || false);
        let mut matcher = fuzzy::Matcher::default();
        for query in ["p", "pF", ""] {
            for _ in 0..3 {
                drop(index.matches(query, &mut matcher));
            }
            let before = (BYTES.load(Ordering::Relaxed), CALLS.load(Ordering::Relaxed));
            let matches = index.matches(query, &mut matcher);
            let bytes = BYTES.load(Ordering::Relaxed) - before.0;
            let calls = CALLS.load(Ordering::Relaxed) - before.1;
            println!("unicode={unicode} query={query} matches={} allocation_bytes={bytes} allocation_calls={calls}",matches.len());
        }
    }
}

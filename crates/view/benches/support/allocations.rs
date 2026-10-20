//! Requested heap allocation volume on the calling thread, excluding fixture setup.
//! This measures allocation calls and requested bytes, not peak memory or RSS.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTS: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}

pub struct Allocator;

fn record(size: usize) {
    let _ = COUNTS.try_with(|counts| {
        if let Some((calls, bytes)) = counts.get() {
            counts.set(Some((calls + 1, bytes + size)));
        }
    });
}

// Forward every allocation to the system allocator with the original layout.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(ptr, layout, size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

pub fn measure(mut operation: impl FnMut()) -> (usize, usize) {
    operation(); // Initialize lazy state before counting.
    COUNTS.with(|counts| counts.set(Some((0, 0))));
    operation();
    COUNTS.with(|counts| counts.replace(None).unwrap())
}

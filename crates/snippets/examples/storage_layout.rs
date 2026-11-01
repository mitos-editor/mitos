//! Report storage affected by SmallVec's union feature on the current target.
use editor_core::{Range, Selection};
use smallvec::SmallVec;
use std::mem::size_of;
fn main() {
    println!("Range: {} bytes", size_of::<Range>());
    println!(
        "SmallVec<[Range; 1]>: {} bytes",
        size_of::<SmallVec<[Range; 1]>>()
    );
    println!("Selection: {} bytes", size_of::<Selection>());
    println!(
        "SmallVec<[usize; 1]>: {} bytes",
        size_of::<SmallVec<[usize; 1]>>()
    );
}

//! Includes discovery, membership indexing and cloning all returned paths.
//! Nucleo result reinjection is measured by the ignored frontend benchmark.
use search::{FileIndex, Options};
use std::{hint::black_box, time::Instant};
fn types() -> ignore::types::Types {
    ignore::types::TypesBuilder::new().build().unwrap()
}
fn main() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    for directory in 0..100 {
        let path = root.join(format!("dir{directory:03}"));
        std::fs::create_dir(&path).unwrap();
        for file in 0..100 {
            std::fs::write(path.join(format!("file{file:03}.rs")), "").unwrap();
        }
    }
    let options = Options {
        parents: false,
        git_global: false,
        ..Options::default()
    };
    let mut index = FileIndex::scan(&root, &options, types(), || false, |_| true).unwrap();
    let target = root.join("dir050/new.rs");
    for incremental in [false, true] {
        let mut samples = Vec::new();
        for _ in 0..31 {
            std::fs::write(&target, "").unwrap();
            let start = Instant::now();
            index = if incremental {
                index
                    .update(std::slice::from_ref(&target), || false)
                    .unwrap()
            } else {
                FileIndex::scan(&root, &options, types(), || false, |_| true).unwrap()
            };
            let rows = index.paths().cloned().collect::<Vec<_>>();
            assert_eq!(black_box(rows).len(), 10001);
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
            std::fs::remove_file(&target).unwrap();
            index = index
                .update(std::slice::from_ref(&target), || false)
                .unwrap();
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "incremental={incremental}, files=10000, median_refresh_ms={:.6}",
            samples[15]
        );
    }
}

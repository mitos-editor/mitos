//! Full directory traversal plus classification. Both paths retain symlink target semantics.
use search::{entry_file_type, Options};
use std::{hint::black_box, time::Instant};
fn main() {
    let fixture = tempfile::tempdir().unwrap();
    for directory in 0..100 {
        let path = fixture.path().join(format!("dir{directory:03}"));
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
    for cached in [false, true] {
        let run = || {
            options
                .walker(fixture.path())
                .build()
                .filter_map(Result::ok)
                .filter(|entry| {
                    if cached {
                        entry_file_type(entry).is_some_and(|kind| kind.is_file())
                    } else {
                        entry.path().is_file()
                    }
                })
                .count()
        };
        assert_eq!(run(), 10_000);
        for _ in 0..3 {
            black_box(run());
        }
        let mut samples = (0..21)
            .map(|_| {
                let start = Instant::now();
                black_box(run());
                start.elapsed().as_secs_f64() * 1000.0
            })
            .collect::<Vec<_>>();
        samples.sort_by(f64::total_cmp);
        println!("cached={cached}, files=10000, median_ms={:.6}", samples[10]);
    }
}

//! Blocking workspace scanning over explicit inputs and a replaceable cache.
//!
//! No editor, callback destination, or asynchronous scheduling belongs here.
use arc_swap::ArcSwap;
use dashmap::DashMap;
use editor_core::Rope;
use grep_regex::RegexMatcher;
use grep_searcher::{sinks, BinaryDetection, SearcherBuilder};
use ignore::{DirEntry, WalkBuilder, WalkState};
use language::{syntax::Loader, tags::Tag, Syntax};
use std::{
    collections::HashSet,
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use stdx::rope;
use text_io::from_reader;

type SyntaxCache = DashMap<PathBuf, Option<(Rope, Syntax)>>;

pub(super) struct Worker {
    root: PathBuf,
    walker: WalkBuilder,
    cache: ArcSwap<SyntaxCache>,
}

impl Worker {
    pub(super) fn new(root: PathBuf, options: search::Options) -> Self {
        Self {
            walker: options.walker(&root),
            root,
            cache: ArcSwap::from_pointee(DashMap::default()),
        }
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    pub(super) fn invalidate(&self) {
        // Prepared scans retain their old cache and cannot refill the replacement.
        self.cache.store(Arc::new(DashMap::default()));
    }

    pub(super) fn prepare(
        &self,
        loader: Arc<Loader>,
        documents: HashSet<PathBuf>,
        pattern: rope::Regex,
        matcher: RegexMatcher,
    ) -> Scan {
        Scan {
            walker: self.walker.clone(),
            cache: self.cache.load_full(),
            loader,
            documents,
            pattern,
            matcher,
        }
    }
}

pub(super) struct Scan {
    walker: WalkBuilder,
    cache: Arc<SyntaxCache>,
    loader: Arc<Loader>,
    documents: HashSet<PathBuf>,
    pattern: rope::Regex,
    matcher: RegexMatcher,
}

impl Scan {
    pub(super) fn run(
        self,
        emit: impl Fn(Tag, &Path) -> ControlFlow<()> + Sync,
        is_cancelled: impl Fn() -> bool + Sync,
    ) {
        let stopped = AtomicBool::new(false);
        let cancelled = || stopped.load(Ordering::Relaxed) || is_cancelled();
        let searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .build();
        self.walker.build_parallel().run(|| {
            let mut searcher = searcher.clone();
            let matcher = &self.matcher;
            let emit = &emit;
            let cancelled = &cancelled;
            let stopped = &stopped;

            let loader = self.loader.clone();
            let documents = &self.documents;
            let pattern = &self.pattern;
            let syntax_cache = self.cache.clone();
            Box::new(move |entry: Result<DirEntry, ignore::Error>| -> WalkState {
                if cancelled() {
                    return WalkState::Quit;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => return WalkState::Continue,
                };
                if !entry.path().is_file() {
                    return WalkState::Continue;
                }
                let path = entry.path();
                // If this document is open, skip it because we've already processed it above.
                if documents.contains(path) {
                    return WalkState::Continue;
                };
                let mut quit = false;
                let sink = sinks::UTF8(|_line, _content| {
                    if !syntax_cache.contains_key(path) {
                        // Read the file into a Rope and attempt to recognize the language
                        // and parse it with tree-sitter. Save the Rope and Syntax for future
                        // queries.
                        syntax_cache.insert(path.to_path_buf(), syntax_for_path(path, &loader));
                    };
                    let entry = syntax_cache.get(path).unwrap();
                    let Some((text, syntax)) = entry.value() else {
                        // If the file couldn't be parsed, move on.
                        return Ok(false);
                    };
                    for tag in language::tags::tags(syntax, &loader, text.slice(..), Some(pattern))
                    {
                        if cancelled() {
                            quit = true;
                            break;
                        }
                        if emit(tag, path).is_break() {
                            stopped.store(true, Ordering::Relaxed);
                            quit = true;
                            break;
                        }
                    }
                    // Quit after seeing the first regex match. We only care to find files
                    // that contain the pattern and then we run the tags query within
                    // those. The location and contents of a match are irrelevant - it's
                    // only important _if_ a file matches.
                    Ok(false)
                });
                if let Err(err) = searcher.search_path(matcher, path, sink) {
                    log::info!("Workspace syntax search error: {}, {}", path.display(), err);
                }
                if quit {
                    WalkState::Quit
                } else {
                    WalkState::Continue
                }
            })
        });
    }
}

/// Create a Rope and language config for a given existing path without creating a full Document.
fn syntax_for_path(path: &Path, loader: &Loader) -> Option<(Rope, Syntax)> {
    let mut file = std::fs::File::open(path).ok()?;
    let (rope, _encoding, _has_bom) = from_reader(&mut file, None).ok()?;
    let text = rope.slice(..);
    let language = loader
        .language_for_filename(path)
        .or_else(|| loader.language_for_shebang(text))?;
    Syntax::new(text, language, loader)
        .ok()
        .map(|syntax| (rope, syntax))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn loader() -> Arc<Loader> {
        Arc::new(
            Loader::new(
                toml::from_str(
                    r#"[[language]]
name = "rust"
scope = "source.rust"
file-types = ["rs"]
roots = []
"#,
                )
                .unwrap(),
                loader::syntax::Resources::default(),
            )
            .unwrap(),
        )
    }

    fn prepare(worker: &Worker) -> Scan {
        let (pattern, matcher) = super::super::query_patterns("sample", true).unwrap();
        worker.prepare(loader(), HashSet::new(), pattern, matcher)
    }

    fn names(scan: Scan) -> Vec<String> {
        let names = Mutex::new(Vec::new());
        scan.run(
            |tag, _| {
                names.lock().unwrap().push(tag.name);
                ControlFlow::Continue(())
            },
            || false,
        );
        names.into_inner().unwrap()
    }

    #[test]
    fn prepared_scans_cannot_refill_a_replacement_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.rs");
        std::fs::write(&path, "fn sample_old() {}\n").unwrap();
        let worker = Worker::new(dir.path().to_owned(), search::Options::default());
        let old = prepare(&worker);
        worker.invalidate();
        assert_eq!(names(old), ["sample_old"]);
        std::fs::write(&path, "fn sample_new() {}\n").unwrap();
        assert_eq!(names(prepare(&worker)), ["sample_new"]);
    }

    #[test]
    fn a_stopped_sink_ends_scanning_and_cancelled_scans_emit_nothing() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            std::fs::write(dir.path().join(format!("{i}.rs")), "fn sample_name() {}\n").unwrap();
        }
        let mut worker = Worker::new(dir.path().to_owned(), search::Options::default());
        worker.walker.threads(1);
        let emitted = std::sync::atomic::AtomicUsize::new(0);
        prepare(&worker).run(
            |_, _| {
                emitted.fetch_add(1, Ordering::Relaxed);
                ControlFlow::Break(())
            },
            || false,
        );
        assert_eq!(emitted.load(Ordering::Relaxed), 1);
        prepare(&worker).run(|_, _| panic!("canceled scan emitted a symbol"), || true);
    }
}

//! Workspace syntax-symbol search over explicit resources and a replaceable parsed-file cache.
//!
//! [`Query`] compiles consistent smart-case matching for source text and symbol names.
//! [`Index`] owns traversal policy and cache generations; prepared [`Scan`]s own their
//! snapshots and run synchronously. Applications supply roots, language resources,
//! excluded open-file paths, cancellation, and result delivery, and choose the worker
//! on which to run scans. This crate has no editor, frontend, event, or protocol state.
use arc_swap::ArcSwap;
use dashmap::DashMap;
use editor_core::Rope;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
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

/// Regex query with one case policy for symbol matching and on-disk filtering.
pub struct Query {
    pattern: rope::Regex,
    prefilter: RegexMatcher,
}

impl Query {
    /// Compile both patterns before any results are emitted. Uppercase queries are
    /// case sensitive; lowercase queries ignore case when `smart_case` is enabled.
    /// Explicit regex flags can override this default, as in buffer text search.
    pub fn new(query: &str, smart_case: bool) -> anyhow::Result<Self> {
        let case_insensitive = smart_case && !query.chars().any(char::is_uppercase);
        let pattern = rope::RegexBuilder::new()
            .syntax(rope::Config::new().case_insensitive(case_insensitive))
            .build(query)?;
        let prefilter = RegexMatcherBuilder::new()
            .case_insensitive(case_insensitive)
            .build(query)?;
        Ok(Self { pattern, prefilter })
    }

    /// Pattern for matching symbols from caller-owned syntax trees and unsaved text.
    pub fn pattern(&self) -> &rope::Regex {
        &self.pattern
    }
}

type SyntaxCache = DashMap<PathBuf, Option<(Rope, Syntax)>>;

/// Workspace root, traversal policy, and parsed-file cache shared across queries.
pub struct Index {
    root: PathBuf,
    walker: WalkBuilder,
    cache: ArcSwap<SyntaxCache>,
}

impl Index {
    /// Create an index without reading files or loading syntax resources.
    pub fn new(root: PathBuf, options: search::Options) -> Self {
        Self {
            walker: options.walker(&root),
            root,
            cache: ArcSwap::from_pointee(DashMap::default()),
        }
    }

    /// Root directory used for traversal.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Replace cached syntax. Previously prepared scans retain their old generation.
    pub fn invalidate(&self) {
        // Prepared scans retain their old cache and cannot refill the replacement.
        self.cache.store(Arc::new(DashMap::default()));
    }

    /// Capture a language loader, open paths to exclude, a query, and a cache generation.
    /// The scan borrows nothing from this index and can be sent to a blocking worker.
    pub fn prepare(&self, loader: Arc<Loader>, documents: HashSet<PathBuf>, query: Query) -> Scan {
        Scan {
            walker: self.walker.clone(),
            cache: self.cache.load_full(),
            loader,
            documents,
            query,
        }
    }
}

/// Owned scan inputs. Preparing a scan does not start filesystem work.
pub struct Scan {
    walker: WalkBuilder,
    cache: Arc<SyntaxCache>,
    loader: Arc<Loader>,
    documents: HashSet<PathBuf>,
    query: Query,
}

impl Scan {
    /// Stream symbol tags with their source paths. Observe cancellation during traversal
    /// and tag publication; a sink returning `Break` stops the scan. In-flight callbacks
    /// on other threads may already be running. Unreadable or unparseable files are skipped.
    pub fn run(
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
            let matcher = &self.query.prefilter;
            let emit = &emit;
            let cancelled = &cancelled;
            let stopped = &stopped;

            let loader = self.loader.clone();
            let documents = &self.documents;
            let pattern = &self.query.pattern;
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

    fn prepare(worker: &Index) -> Scan {
        worker.prepare(
            loader(),
            HashSet::new(),
            Query::new("sample", true).unwrap(),
        )
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
        let worker = Index::new(dir.path().to_owned(), search::Options::default());
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
        let mut worker = Index::new(dir.path().to_owned(), search::Options::default());
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

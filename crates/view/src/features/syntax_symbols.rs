//! Syntax-symbol queries, parsed-file caches, and workspace refresh ownership.
use super::workspace_changes::{Documents, Policy, Polling, WorkspaceChanges};
use crate::{
    callbacks::EditorCallbackSender,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    Document, DocumentId, Editor,
};
use anyhow::Result;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use editor_core::{Rope, RopeSlice, Selection, Uri};
use futures_util::{future::BoxFuture, FutureExt};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{sinks, BinaryDetection, SearcherBuilder};
use ignore::{DirEntry, WalkBuilder, WalkState};
use language::{
    syntax::Loader,
    tags::{Tag, TagKind},
    Syntax,
};
use parking_lot::Mutex;
use std::{
    collections::HashSet,
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::Arc,
};
use stdx::{path, rope};
use text_io::from_reader;

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Uri(Uri),
    Document(DocumentId),
}

#[derive(Debug, PartialEq)]
pub struct Symbol {
    pub tag: Tag,
    pub target: Target,
}

impl Symbol {
    pub fn key(&self) -> (Result<PathBuf, DocumentId>, TagKind, String) {
        let target = match &self.target {
            Target::Uri(uri) => Ok(uri.as_path().unwrap().to_owned()),
            Target::Document(id) => Err(*id),
        };
        (target, self.tag.kind, self.tag.name.clone())
    }

    pub fn quicklist_entry(&self) -> Option<QuicklistEntry> {
        let target = match &self.target {
            Target::Uri(uri) => QuicklistTarget::Path(uri.as_path()?.to_owned()),
            Target::Document(id) => QuicklistTarget::Document(*id),
        };
        Some(QuicklistEntry {
            target,
            position: QuicklistPosition::Selection(Selection::single(self.tag.start, self.tag.end)),
        })
    }
}

fn tags_iter<'a>(
    syntax: &'a Syntax,
    loader: &'a Loader,
    text: RopeSlice<'a>,
    target: Target,
    pattern: Option<&'a rope::Regex>,
) -> impl Iterator<Item = Symbol> + 'a {
    language::tags::tags(syntax, loader, text, pattern).map(move |tag| Symbol {
        tag,
        target: target.clone(),
    })
}

pub fn document_symbols(editor: &Editor, document: DocumentId) -> Vec<Symbol> {
    let Some(doc) = editor.document(document) else {
        return Vec::new();
    };
    let loader = editor.syn_loader.load();
    doc.syntax()
        .map(|syntax| {
            tags_iter(
                syntax,
                &loader,
                doc.text().slice(..),
                Target::Document(document),
                None,
            )
            .collect()
        })
        .unwrap_or_default()
}

pub struct WorkspaceSymbols {
    owner: EditorCallbackSender,
    searcher_builder: SearcherBuilder,
    walk_builder: WalkBuilder,
    smart_case: bool,
    search_root: PathBuf,
    syntax_cache: ArcSwap<DashMap<PathBuf, Option<(Rope, Syntax)>>>,
    refresh: Mutex<WorkspaceChanges>,
}

impl WorkspaceSymbols {
    pub fn new(editor: &Editor, document: DocumentId, options: search::Options) -> Self {
        let search_root = editor
            .document(document)
            .and_then(Document::path)
            .map(|path| workspace::find_workspace_in(path).0)
            .unwrap_or_else(|| workspace::find_workspace().0);
        let mut searcher_builder = SearcherBuilder::new();
        searcher_builder.binary_detection(BinaryDetection::quit(b'\x00'));
        let refresh = Mutex::new(WorkspaceChanges::new(
            editor,
            &search_root,
            Policy {
                contents: true,
                documents: Documents::All,
                polling: Polling::Unwatched,
                ignore_files: true,
                ..Policy::default()
            },
        ));
        Self {
            owner: editor.callback_sender().clone(),
            searcher_builder,
            walk_builder: options.walker(&search_root),
            smart_case: editor.config().search.smart_case,
            search_root,
            syntax_cache: ArcSwap::from_pointee(DashMap::default()),
            refresh,
        }
    }

    pub fn root(&self) -> &Path {
        &self.search_root
    }

    /// Emit open-buffer symbols now, then scan remaining files on a blocking worker.
    /// The caller's cancellation predicate must cover replacement and closure of its query.
    pub fn query(
        self: &Arc<Self>,
        editor: &mut Editor,
        query: &str,
        emit: impl Fn(Symbol) -> ControlFlow<()> + Send + Sync + 'static,
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> BoxFuture<'static, Result<()>> {
        if !self.owner.same_destination(editor.callback_sender()) {
            return async { anyhow::bail!("Syntax symbol session belongs to another editor") }
                .boxed();
        }
        let state = self.clone();
        let emit = Arc::new(emit);
        let cancelled = Arc::new(cancelled);
        if cancelled() || query.len() < 3 {
            return async { Ok(()) }.boxed();
        }
        // Attempt to find the tag in any open documents.
        let (pattern, matcher) = match query_patterns(query, state.smart_case) {
            Ok(patterns) => patterns,
            Err(err) => return async { Err(err) }.boxed(),
        };
        editor.clear_status();
        let pending: Vec<_> = editor
            .documents()
            .filter(|doc| doc.is_syntax_pending())
            .map(Document::id)
            .collect();
        for id in pending {
            editor.ensure_syntax(id);
        }
        let loader = editor.syn_loader.load();
        for doc in editor.documents() {
            let Some(syntax) = doc.syntax() else { continue };
            let text = doc.text().slice(..);
            let uri_or_id = doc
                .uri()
                .map(Target::Uri)
                .unwrap_or_else(|| Target::Document(doc.id()));
            for tag in tags_iter(syntax, &loader, text.slice(..), uri_or_id, Some(&pattern)) {
                if cancelled() || emit(tag).is_break() {
                    return async { Ok(()) }.boxed();
                }
            }
        }
        if !state.search_root.exists() {
            return async { Err(anyhow::anyhow!("Current working directory does not exist")) }
                .boxed();
        }
        let pattern = Arc::new(pattern);

        let loader = editor.syn_loader.load();
        let syntax_cache = state.syntax_cache.load_full();

        let documents: HashSet<_> = editor
            .documents()
            .filter_map(Document::path)
            .map(ToOwned::to_owned)
            .collect();

        async move {
            tokio::task::spawn_blocking(move || {
                let searcher = state.searcher_builder.build();
                state.walk_builder.build_parallel().run(|| {
                    let mut searcher = searcher.clone();
                    let matcher = matcher.clone();
                    let emit = emit.clone();
                    let cancelled = cancelled.clone();

                    let loader = loader.clone();
                    let documents = &documents;
                    let pattern = pattern.clone();
                    let syntax_cache = syntax_cache.clone();
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
                                syntax_cache
                                    .insert(path.to_path_buf(), syntax_for_path(path, &loader));
                            };
                            let entry = syntax_cache.get(path).unwrap();
                            let Some((text, syntax)) = entry.value() else {
                                // If the file couldn't be parsed, move on.
                                return Ok(false);
                            };
                            let uri = Uri::from(path::normalize(path));
                            for tag in tags_iter(
                                syntax,
                                &loader,
                                text.slice(..),
                                Target::Uri(uri),
                                Some(&pattern),
                            ) {
                                if cancelled() || emit(tag).is_break() {
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
                        if let Err(err) = searcher.search_path(&matcher, path, sink) {
                            log::info!(
                                "Workspace syntax search error: {}, {}",
                                path.display(),
                                err
                            );
                        }
                        if quit {
                            WalkState::Quit
                        } else {
                            WalkState::Continue
                        }
                    })
                });
            })
            .await?;
            Ok(())
        }
        .boxed()
    }

    /// Invalidate caches when document snapshots or observed workspace files change.
    pub fn refresh(&self, editor: &Editor) -> bool {
        if !self.owner.same_destination(editor.callback_sender()) {
            return false;
        }
        let mut refresh = self.refresh.lock();
        let changed = refresh.changed(editor, &self.search_root) || refresh.poll_due(editor);
        if changed {
            refresh.mark_refreshed();
            // Old workers retain their old cache and cannot repopulate the new one.
            self.syntax_cache.store(Arc::new(DashMap::default()));
        }
        changed
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

// Apply the same case policy to tag matching and the on-disk file prefilter.
// As with buffer search, explicit regex flags can override this default.
fn query_patterns(query: &str, smart_case: bool) -> Result<(rope::Regex, RegexMatcher)> {
    let case_insensitive = smart_case && !query.chars().any(char::is_uppercase);
    let pattern = rope::RegexBuilder::new()
        .syntax(rope::Config::new().case_insensitive(case_insensitive))
        .build(query)?;
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive)
        .build(query)?;
    Ok((pattern, matcher))
}

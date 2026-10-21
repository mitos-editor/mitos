//! Editor-facing syntax-symbol queries and workspace refresh ownership.
mod worker;
use super::workspace_changes::{Documents, Policy, Polling, WorkspaceChanges};
use crate::{
    callbacks::EditorCallbackSender,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    Document, DocumentId, Editor,
};
use anyhow::Result;
use editor_core::{RopeSlice, Selection, Uri};
use futures_util::{future::BoxFuture, FutureExt};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
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
};
use stdx::{path, rope};

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
    worker: worker::Worker,
    smart_case: bool,
    refresh: Mutex<WorkspaceChanges>,
}

impl WorkspaceSymbols {
    pub fn new(editor: &Editor, document: DocumentId, options: search::Options) -> Self {
        let search_root = editor
            .document(document)
            .and_then(Document::path)
            .map(|path| workspace::find_workspace_in(path).0)
            .unwrap_or_else(|| workspace::find_workspace().0);
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
            worker: worker::Worker::new(search_root, options),
            smart_case: editor.config().search.smart_case,
            refresh,
        }
    }

    pub fn root(&self) -> &Path {
        self.worker.root()
    }

    /// Emit open-buffer symbols now, then scan remaining files on a blocking worker.
    /// The caller's cancellation predicate must cover replacement and closure of its query.
    pub fn query(
        &self,
        editor: &mut Editor,
        query: &str,
        emit: impl Fn(Symbol) -> ControlFlow<()> + Send + Sync + 'static,
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> BoxFuture<'static, Result<()>> {
        if !self.owner.same_destination(editor.callback_sender()) {
            return async { anyhow::bail!("Syntax symbol session belongs to another editor") }
                .boxed();
        }
        if cancelled() || query.len() < 3 {
            return async { Ok(()) }.boxed();
        }
        // Attempt to find the tag in any open documents.
        let (pattern, matcher) = match query_patterns(query, self.smart_case) {
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
        if !self.worker.root().exists() {
            return async { Err(anyhow::anyhow!("Current working directory does not exist")) }
                .boxed();
        }

        let loader = editor.syn_loader.load_full();

        let documents: HashSet<_> = editor
            .documents()
            .filter_map(Document::path)
            .map(ToOwned::to_owned)
            .collect();

        let scan = self.worker.prepare(loader, documents, pattern, matcher);
        async move {
            tokio::task::spawn_blocking(move || {
                scan.run(
                    move |tag, path| {
                        emit(Symbol {
                            tag,
                            target: Target::Uri(Uri::from(path::normalize(path))),
                        })
                    },
                    cancelled,
                );
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
        let changed = refresh.changed(editor, self.worker.root()) || refresh.poll_due(editor);
        if changed {
            refresh.mark_refreshed();
            self.worker.invalidate();
        }
        changed
    }
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

//! Fixed historical inputs. Repository reads and parsing share the review worker budget.
use super::{decode, engine::Settings, Review, ReviewSource};
use crate::{editor::Action, Document, DocumentId, Editor};
use anyhow::{ensure, Result};
use editor_core::Rope;
use std::{path::PathBuf, sync::Arc};
#[cfg(feature = "git")]
use vcs::operations::{history::RevisionFile, GitRepository};

#[cfg(feature = "git")]
#[derive(Clone)]
pub(crate) struct HistoricalPermalink {
    pub path: PathBuf,
    pub revision: vcs::operations::history::RevisionId,
}

#[derive(Clone)]
enum Input {
    #[cfg(feature = "git")]
    Revisions {
        repository: GitRepository,
        before: Option<RevisionFile>,
        after: Option<RevisionFile>,
        buffer: Option<Rope>,
    },
    Snapshot(Arc<vcs::Comparison>),
    #[cfg(feature = "git")]
    Stash {
        repository: vcs::operations::GitRepository,
        entry: vcs::operations::StashEntry,
        file: vcs::operations::StashFile,
    },
}

/// An immutable comparison whose completion belongs to its native document.
#[derive(Clone)]
pub struct HistoricalReviewRequest {
    pub(super) path: PathBuf,
    pub(super) workspace: PathBuf,
    pub(super) label: String,
    pub(super) settings: Settings,
    before_path: PathBuf,
    before_label: String,
    after_label: String,
    before_settings: Settings,
    trusted: bool,
    input: Input,
    worker: Arc<tokio::sync::Semaphore>,
}

/// A bounded historical preview that can be reused by a native review document.
pub struct HistoricalReviewSnapshot {
    pub source: Arc<ReviewSource>,
    pub review: Arc<Review>,
    pub(super) request: HistoricalReviewRequest,
}

impl HistoricalReviewRequest {
    /// The actual Git endpoint behind a historical side, never a shortened label.
    #[cfg(feature = "git")]
    pub(crate) fn permalink_source(&self, before_side: bool) -> Option<HistoricalPermalink> {
        match &self.input {
            Input::Revisions {
                before,
                after,
                buffer,
                repository,
                ..
            } => {
                let endpoint = if before_side || buffer.is_some() {
                    before.as_ref()
                } else {
                    after.as_ref()
                }?;
                Some(HistoricalPermalink {
                    path: repository.root().join(endpoint.path()),
                    revision: endpoint.revision.clone(),
                })
            }
            _ => None,
        }
    }
    /// Capture preloaded immutable sources from any VCS provider.
    pub fn comparison(
        editor: &Editor,
        path: PathBuf,
        workspace: PathBuf,
        label: String,
        comparison: vcs::Comparison,
    ) -> Self {
        Self {
            settings: Settings::historical(editor, &path),
            worker: editor.handlers.change_review.worker.clone(),
            before_settings: Settings::historical(editor, &path),
            before_path: path.clone(),
            before_label: label.clone(),
            after_label: label.clone(),
            trusted: editor
                .workspace_trust
                .query(&workspace, workspace::trust::TrustQuery::Git)
                .is_trusted(),
            path,
            workspace,
            label,
            input: Input::Snapshot(Arc::new(comparison)),
        }
    }

    #[cfg(feature = "git")]
    pub fn stash(
        editor: &Editor,
        repository: vcs::operations::GitRepository,
        entry: vcs::operations::StashEntry,
        file: vcs::operations::StashFile,
    ) -> Self {
        let path = repository.root().join(file.path());
        let workspace = repository.root().to_owned();
        Self {
            settings: Settings::historical(editor, &path),
            worker: editor.handlers.change_review.worker.clone(),
            before_settings: Settings::historical(editor, &path),
            before_path: path.clone(),
            before_label: format!("{}^1", &entry.id()[..12]),
            after_label: format!("{} {}", &entry.id()[..12], file.layer().label()),
            label: format!("{} {}", &entry.id()[..12], file.layer().label()),
            trusted: editor
                .workspace_trust
                .query(&workspace, workspace::trust::TrustQuery::Git)
                .is_trusted(),
            path,
            workspace,
            input: Input::Stash {
                repository,
                entry,
                file,
            },
        }
    }

    #[cfg(feature = "git")]
    pub fn revisions(
        editor: &Editor,
        repository: GitRepository,
        before: Option<RevisionFile>,
        after: Option<RevisionFile>,
    ) -> Result<Self> {
        ensure!(
            before.is_some() || after.is_some(),
            "A historical file is required"
        );
        let endpoint = after.as_ref().or(before.as_ref()).unwrap();
        let path = repository.root().join(endpoint.path());
        let before_path = before
            .as_ref()
            .map_or_else(|| path.clone(), |file| repository.root().join(file.path()));
        let before_label = before
            .as_ref()
            .map_or("empty", |file| file.revision.short())
            .to_owned();
        let after_label = after
            .as_ref()
            .map_or("deleted", |file| file.revision.short())
            .to_owned();
        let workspace = repository.root().to_owned();
        Ok(Self {
            settings: Settings::historical(editor, &path),
            worker: editor.handlers.change_review.worker.clone(),
            before_settings: Settings::historical(editor, &before_path),
            before_path,
            trusted: editor
                .workspace_trust
                .query(&workspace, workspace::trust::TrustQuery::Git)
                .is_trusted(),
            label: format!("{before_label} → {after_label}"),
            before_label,
            after_label,
            path,
            workspace,
            input: Input::Revisions {
                repository,
                before,
                after,
                buffer: None,
            },
        })
    }

    #[cfg(feature = "git")]
    pub fn against_buffer(
        editor: &Editor,
        repository: GitRepository,
        before: RevisionFile,
        buffer: DocumentId,
    ) -> Result<Self> {
        let doc = editor
            .document(buffer)
            .ok_or_else(|| anyhow::anyhow!("The original buffer has been closed"))?;
        ensure!(!doc.is_binary(), "A text buffer is required");
        let path = doc
            .path()
            .ok_or_else(|| anyhow::anyhow!("The original file buffer is unavailable"))?
            .to_owned();
        ensure!(
            doc.text().len() <= vcs::MAX_REVIEW_BYTES,
            "Buffer too large to compare (limit 10 MiB)"
        );
        let mut request = Self::revisions(editor, repository.clone(), Some(before.clone()), None)?;
        request.path = stdx::path::canonicalize_existing(&path);
        request.settings = Settings::capture(editor, &path);
        request.after_label = "buffer snapshot".into();
        request.label = format!("{} → buffer", before.revision.short());
        request.input = Input::Revisions {
            repository,
            before: Some(before),
            after: None,
            buffer: Some(doc.text().clone()),
        };
        Ok(request)
    }

    pub fn engine(&self) -> super::DiffEngine {
        self.settings.engine
    }

    /// Refresh comparison policy without replacing pinned revisions or captured buffer text.
    pub(super) fn refresh_settings(&mut self, editor: &Editor) {
        self.settings = Settings::historical(editor, &self.path);
        #[cfg(feature = "git")]
        if matches!(
            &self.input,
            Input::Revisions {
                buffer: Some(_),
                ..
            }
        ) {
            self.settings = Settings::capture(editor, &self.path);
        }
        self.before_settings = Settings::historical(editor, &self.before_path);
    }

    /// Compare configured review policy independently of the immutable source key.
    pub fn same_policy(&self, other: &Self) -> bool {
        self.trusted == other.trusted
            && self.settings.current(&other.settings)
            && self.before_settings.current(&other.before_settings)
    }

    /// Historical previews share the editor's bounded, cancellable review worker.
    pub async fn run(self, cancel: event::TaskHandle) -> Result<Arc<HistoricalReviewSnapshot>> {
        let worker = self.worker.clone();
        let check = cancel.clone();
        super::run_review(worker, cancel, move || {
            let (source, review) = self.compute(&|| check.is_canceled())?;
            ensure!(!check.is_canceled(), "Historical review canceled");
            Ok(Arc::new(HistoricalReviewSnapshot {
                source,
                review,
                request: self,
            }))
        })
        .await
    }

    pub(super) fn source_metadata(&self, before: bool) -> (PathBuf, String) {
        if before {
            (self.before_path.clone(), self.before_label.clone())
        } else {
            (self.path.clone(), self.after_label.clone())
        }
    }

    pub(super) fn is_current(&self, editor: &Editor) -> bool {
        self.trusted
            == editor
                .workspace_trust
                .query(&self.workspace, workspace::trust::TrustQuery::Git)
                .is_trusted()
    }

    pub(super) fn compute(
        &self,
        canceled: &dyn Fn() -> bool,
    ) -> Result<(Arc<ReviewSource>, Arc<Review>)> {
        let source = self.source(canceled)?;
        let review = source.compute(None, canceled);
        Ok((source, review))
    }

    fn source(&self, canceled: &dyn Fn() -> bool) -> Result<Arc<ReviewSource>> {
        ensure!(!canceled(), "Historical review canceled");
        let comparison: vcs::Comparison = match &self.input {
            #[cfg(feature = "git")]
            Input::Revisions {
                repository,
                before,
                after,
                ..
            } => vcs::Comparison {
                before: before
                    .as_ref()
                    .map(|file| repository.read_revision_file(file, self.trusted))
                    .transpose()?
                    .unwrap_or_default(),
                after: after
                    .as_ref()
                    .map(|file| repository.read_revision_file(file, self.trusted))
                    .transpose()?
                    .unwrap_or_default(),
            },
            Input::Snapshot(comparison) => vcs::Comparison {
                before: comparison.before.clone(),
                after: comparison.after.clone(),
            },
            #[cfg(feature = "git")]
            Input::Stash {
                repository,
                entry,
                file,
            } => repository.read_stash_file(entry, file, self.trusted)?,
        };
        ensure!(!canceled(), "Historical review canceled");
        // Historical encodings are detected from the historical bytes, independently of today's file.
        let after = {
            #[cfg(feature = "git")]
            if let Input::Revisions {
                buffer: Some(buffer),
                ..
            } = &self.input
            {
                buffer.clone()
            } else {
                decode(&comparison.after, None)?
            }
            #[cfg(not(feature = "git"))]
            decode(&comparison.after, None)?
        };
        let source = Arc::new(
            ReviewSource::new(
                decode(&comparison.before, None)?,
                after,
                None,
                self.settings.clone(),
            )
            .with_before_settings(self.before_settings.clone()),
        );
        Ok(source)
    }
}

pub(crate) struct HistoricalDocument {
    pub path: PathBuf,
    pub workspace: PathBuf,
    pub label: String,
    #[cfg(feature = "git")]
    pub revision: Option<HistoricalPermalink>,
    _task: event::TaskController,
}

impl Editor {
    pub(super) fn open_historical_source(
        &mut self,
        request: &HistoricalReviewRequest,
        source: Arc<ReviewSource>,
        action: Action,
        before: bool,
    ) -> DocumentId {
        let (path, label) = request.source_metadata(before);
        let mut doc = Document::from(
            source.text(before).clone(),
            None,
            self.config.clone(),
            source.loader(),
        );
        // Syntax was prepared off-thread for this exact snapshot. It carries no filesystem/LSP identity.
        doc.historical = Some(HistoricalDocument {
            path,
            workspace: request.workspace.clone(),
            label,
            #[cfg(feature = "git")]
            revision: request.permalink_source(before),
            _task: event::TaskController::new(),
        });
        let id = self.new_file_from_document(action, doc);
        source.initialize_document(self.document_mut(id).unwrap(), before);
        event::dispatch(crate::events::DocumentDidOpen {
            editor: self,
            doc: id,
        })
        .report_to(self.status_sender());
        id
    }
}

impl Editor {
    /// Open a loading source buffer immediately; completion only updates this owner.
    pub fn open_historical_version(
        &mut self,
        request: HistoricalReviewRequest,
        action: Action,
    ) -> DocumentId {
        let (path, label) = request.source_metadata(false);
        let mut doc = Document::from(
            Rope::from("Loading historical version…\n"),
            None,
            self.config.clone(),
            self.syn_loader.load_full(),
        );
        let mut task = event::TaskController::new();
        let cancel = task.restart();
        doc.historical = Some(HistoricalDocument {
            path,
            workspace: request.workspace.clone(),
            label,
            #[cfg(feature = "git")]
            revision: request.permalink_source(false),
            _task: task,
        });
        let id = self.new_file_from_document(action, doc);
        event::dispatch(crate::events::DocumentDidOpen {
            editor: self,
            doc: id,
        })
        .report_to(self.status_sender());
        let old = self.document(id).unwrap().snapshot();
        let worker = self.handlers.change_review.worker.clone();
        let callbacks = self.callback_sender().clone();
        let check = cancel.clone();
        let before = old.text().clone();
        tokio::spawn(async move {
            let captured = request.clone();
            let result = super::run_review(worker, cancel.clone(), move || {
                let source = captured.source(&|| check.is_canceled())?;
                source.prepare_syntax(&|| check.is_canceled());
                ensure!(!check.is_canceled(), "Historical source canceled");
                let transaction = text_diff::compare_ropes(&before, source.after());
                Ok((source, transaction))
            })
            .await;
            if cancel.is_canceled() {
                return;
            }
            callbacks
                .send(move |editor| {
                    if cancel.is_canceled() {
                        return;
                    }
                    let result = if request.is_current(editor) {
                        result
                    } else {
                        Err(anyhow::anyhow!(
                            "Workspace trust changed; reopen this historical version"
                        ))
                    };
                    let Some(doc) = editor.document_mut(id) else {
                        return;
                    };
                    if !old.is_current(&doc.buffer) {
                        return;
                    }
                    let (source, transaction) = match result {
                        Ok((source, transaction)) => (Some(source), transaction),
                        Err(error) => {
                            let text = format!("Cannot open historical version: {error}\n");
                            (
                                None,
                                editor_core::Transaction::change(
                                    doc.text(),
                                    [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
                                ),
                            )
                        }
                    };
                    doc.replace_readonly_content(transaction.changes(), true)
                        .expect("historical publication uses the captured snapshot");
                    if let Some(source) = source {
                        source.initialize_document(doc, false);
                    }
                    editor.redraw().request();
                })
                .await;
        });
        id
    }
}

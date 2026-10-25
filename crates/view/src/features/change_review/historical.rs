//! Fixed historical inputs. Repository reads and parsing share the review worker budget.
use super::{decode, engine::Settings, Review, ReviewSource};
use crate::{editor::Action, Document, DocumentId, Editor};
use anyhow::{ensure, Result};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
enum Input {
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
    trusted: bool,
    input: Input,
}

impl HistoricalReviewRequest {
    /// Capture preloaded immutable sources from any VCS provider.
    pub fn comparison(
        editor: &Editor,
        path: PathBuf,
        workspace: PathBuf,
        label: String,
        comparison: vcs::Comparison,
    ) -> Self {
        Self {
            settings: Settings::capture(editor, &path),
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
            settings: Settings::capture(editor, &path),
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
        ensure!(!canceled(), "Historical review canceled");
        let comparison: vcs::Comparison = match &self.input {
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
        let source = Arc::new(ReviewSource::new(
            decode(&comparison.before, None)?,
            decode(&comparison.after, None)?,
            None,
            self.settings.clone(),
        ));
        let review = source.compute(None, canceled);
        Ok((source, review))
    }
}

pub(crate) struct HistoricalDocument {
    pub path: PathBuf,
    pub workspace: PathBuf,
    pub label: String,
}

impl Editor {
    pub(super) fn open_historical_source(
        &mut self,
        path: PathBuf,
        workspace: PathBuf,
        label: String,
        source: Arc<ReviewSource>,
        action: Action,
    ) -> DocumentId {
        let mut doc = Document::from(
            source.after().clone(),
            None,
            self.config.clone(),
            source.loader(),
        );
        // Syntax was prepared off-thread for this exact snapshot. It carries no filesystem/LSP identity.
        doc.historical = Some(HistoricalDocument {
            path,
            workspace,
            label,
        });
        let id = self.new_file_from_document(action, doc);
        source.initialize_document(self.document_mut(id).unwrap());
        event::dispatch(crate::events::DocumentDidOpen {
            editor: self,
            doc: id,
        })
        .report_to(self.status_sender());
        id
    }
}

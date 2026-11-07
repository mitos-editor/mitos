//! Editable commit messages paired with the staging area they were composed for.

use super::change_review::ReviewComparison;
use crate::{editor::Action, Document, DocumentId, Editor};
use anyhow::{ensure, Result};
use std::path::{Path, PathBuf};
use vcs::operations::{CommitSnapshot, GitRepository};

pub struct CommitMessage {
    snapshot: CommitSnapshot,
    workspace: PathBuf,
    name: String,
    review: DocumentId,
    submitted: bool,
}

impl CommitMessage {
    pub fn snapshot(&self) -> &CommitSnapshot {
        &self.snapshot
    }
    pub fn repository(&self) -> &GitRepository {
        self.snapshot.repository()
    }
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn review(&self) -> DocumentId {
        self.review
    }
    pub fn submitted(&self) -> bool {
        self.submitted
    }
}

impl Document {
    pub fn commit_message(&self) -> Option<&CommitMessage> {
        self.commit_message.as_deref()
    }
}

impl Editor {
    /// Reopening the composer refreshes staged inputs while retaining any message.
    pub fn open_commit_message(
        &mut self,
        snapshot: CommitSnapshot,
        workspace: PathBuf,
    ) -> Result<DocumentId> {
        ensure!(
            self.workspace_trust
                .query(&workspace, workspace::trust::TrustQuery::Git)
                .is_trusted(),
            "Git commits require workspace trust; run :workspace-trust first"
        );
        let root = snapshot.repository().root().to_owned();
        let existing = self.documents().find_map(|doc| {
            let commit = doc.commit_message()?;
            (!commit.submitted && commit.repository().root() == root)
                .then_some((doc.id(), commit.review))
        });
        let review = if let Some((_, review)) = existing
            && self
                .document(review)
                .is_some_and(|doc| doc.project_review().is_some())
        {
            self.focus_commit_document(review, Action::Replace);
            self.refresh_project_review()?;
            review
        } else {
            self.open_project_review(
                root.clone(),
                workspace.clone(),
                ReviewComparison::Staged,
                Action::Replace,
            )
        };
        let id = if let Some((id, _)) = existing {
            let commit = self
                .document_mut(id)
                .unwrap()
                .commit_message
                .as_mut()
                .unwrap();
            commit.snapshot = snapshot;
            commit.review = review;
            self.focus_commit_document(id, Action::VerticalSplit);
            id
        } else {
            let loader = self.syn_loader.load_full();
            let mut document = Document::default(self.config.clone(), loader.clone());
            let language = loader
                .language_for_name("git-commit")
                .map(|language| loader.language(language).config().clone());
            document.set_language(language, &loader);
            let name = format!("{} [commit message]", root.display())
                .chars()
                .map(|ch| if ch.is_control() { '�' } else { ch })
                .collect();
            document.commit_message = Some(Box::new(CommitMessage {
                snapshot,
                workspace,
                name,
                review,
                submitted: false,
            }));
            self.new_file_from_document(Action::VerticalSplit, document)
        };
        self.set_status("Review staged changes, write a message, then run :vcs-commit-submit");
        Ok(id)
    }

    fn focus_commit_document(&mut self, document: DocumentId, action: Action) {
        let view = self
            .tree
            .views()
            .find_map(|(view, _)| (view.doc == document).then_some(view.id));
        if let Some(view) = view {
            self.focus(view);
        } else {
            self.switch(document, action);
        }
    }

    /// A successful commit must not mark later message edits as submitted or saved.
    pub fn finish_commit_message(&mut self, document: DocumentId, submitted: &buffer::Snapshot) {
        if let Some(doc) = self.document_mut(document)
            && submitted.is_current(&doc.buffer)
            && let Some(commit) = doc.commit_message.as_mut()
        {
            commit.submitted = true;
            doc.commit_changes();
            doc.reset_modified();
        }
    }
}

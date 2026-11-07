//! Document origin owns derived content and its intrinsic restrictions.
//!
//! File association and encoding stay in `FileState`. Review and revision
//! origins are pathless; named virtual sources may retain a file association
//! for consumers such as workspace replacement, but can never be written.

use std::path::Path;

use crate::features::change_review::{historical::HistoricalDocument, ReviewDocument};

use super::Document;

pub(super) enum DocumentSource {
    FileOrScratch,
    Virtual {
        name: String,
    },
    Revision(HistoricalDocument),
    Review(Box<ReviewDocument>),
    ProjectReview {
        name: String,
        state: Box<crate::features::project_review::ProjectReviewDocument>,
    },
}

impl Document {
    /// Original content path, for language detection and repository context.
    /// Unlike [`Self::path`], this may describe immutable repository content;
    /// it must never be used as a save destination or language-server URI.
    pub fn source_path(&self) -> Option<&Path> {
        match &self.source {
            DocumentSource::FileOrScratch => self.path(),
            DocumentSource::Virtual { .. } => self.path(),
            DocumentSource::Revision(source) => Some(&source.path),
            DocumentSource::Review(review) => Some(review.path()),
            DocumentSource::ProjectReview { state, .. } => Some(&state.root),
        }
    }

    /// Repository-derived content has no edit history, modified state, or insert
    /// mode. Named virtual sources retain their existing mode and service policy;
    /// [`Self::is_readonly`] still prevents editing and saving their content.
    pub fn is_content_readonly(&self) -> bool {
        matches!(
            self.source,
            DocumentSource::Revision(_)
                | DocumentSource::Review(_)
                | DocumentSource::ProjectReview { .. }
        )
    }

    /// Intrinsic immutability is independent of filesystem permissions.
    pub(super) fn is_source_readonly(&self) -> bool {
        !matches!(self.source, DocumentSource::FileOrScratch)
    }

    pub fn review(&self) -> Option<&ReviewDocument> {
        match &self.source {
            DocumentSource::Review(review) => Some(review),
            _ => None,
        }
    }

    pub(crate) fn review_mut(&mut self) -> Option<&mut ReviewDocument> {
        match &mut self.source {
            DocumentSource::Review(review) => Some(review),
            _ => None,
        }
    }

    pub fn project_review(
        &self,
    ) -> Option<&crate::features::project_review::ProjectReviewDocument> {
        match &self.source {
            DocumentSource::ProjectReview { state, .. } => Some(state),
            _ => None,
        }
    }

    pub(crate) fn project_review_mut(
        &mut self,
    ) -> Option<&mut crate::features::project_review::ProjectReviewDocument> {
        match &mut self.source {
            DocumentSource::ProjectReview { state, .. } => Some(state),
            _ => None,
        }
    }

    pub(crate) fn set_project_review(
        &mut self,
        name: String,
        state: crate::features::project_review::ProjectReviewDocument,
    ) {
        assert!(
            self.path().is_none(),
            "project review documents must be pathless"
        );
        self.source = DocumentSource::ProjectReview {
            name,
            state: Box::new(state),
        };
    }

    pub(crate) fn historical(&self) -> Option<&HistoricalDocument> {
        match &self.source {
            DocumentSource::Revision(source) => Some(source),
            _ => None,
        }
    }

    pub(crate) fn set_review(&mut self, review: ReviewDocument) {
        assert!(self.path().is_none(), "review documents must be pathless");
        self.source = DocumentSource::Review(Box::new(review));
    }

    pub(crate) fn set_historical(&mut self, source: HistoricalDocument) {
        assert!(self.path().is_none(), "revision documents must be pathless");
        self.source = DocumentSource::Revision(source);
    }

    /// Name an immutable document supplied by an external service.
    pub(crate) fn with_readonly_name(mut self, name: String) -> Self {
        self.source = DocumentSource::Virtual { name };
        self
    }

    pub(crate) fn is_virtual(&self) -> bool {
        matches!(
            self.source,
            DocumentSource::Virtual { .. } | DocumentSource::ProjectReview { .. }
        )
    }

    pub(super) fn virtual_name(&self) -> Option<&str> {
        match &self.source {
            DocumentSource::Virtual { name } | DocumentSource::ProjectReview { name, .. } => {
                Some(name)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{document::DocumentKind, editing::tests::document_and_view};

    #[tokio::test]
    async fn virtual_origin_preserves_file_association_without_allowing_forced_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("original.rs");
        std::fs::write(&path, "original\n").unwrap();
        let (mut doc, _) = document_and_view("#[|content]#\n");
        assert_eq!(doc.display_metadata().kind, DocumentKind::Scratch);
        assert!(!doc.is_readonly());
        doc.set_path(Some(&path));
        assert_eq!(doc.display_metadata().kind, DocumentKind::File);
        assert_eq!(doc.source_path(), doc.path());
        let path = doc.path().unwrap().to_owned();
        let mut doc = doc.with_readonly_name("Adapter source".into());
        assert_eq!(doc.display_metadata().kind, DocumentKind::Virtual);
        assert_eq!(doc.display_name(), "Adapter source");
        assert!(doc.is_readonly());
        assert_eq!(doc.source_path(), Some(path.as_path()));
        assert!(doc.review().is_none() && doc.historical().is_none());
        doc.detect_readonly();
        assert!(
            doc.is_readonly(),
            "filesystem checks cannot remove intrinsic protection"
        );
        let destination = directory.path().join("export.rs");
        assert!(doc.save(Some(destination.clone()), true).is_err());
        assert!(!destination.exists());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original\n");

        let (doc, _) = document_and_view("#[|content]#\n");
        let doc = doc.with_readonly_name("Pathless source".into());
        assert!(doc.is_readonly());
        assert!(doc.path().is_none() && doc.source_path().is_none());
        assert!(doc.uri().is_none() && doc.url().is_none());
    }
}

//! Display identity is separate from a document's filesystem identity.

use std::{borrow::Cow, path::Path};

use crate::{features::change_review::ReviewComparison, Document};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    File,
    Scratch,
    Virtual,
    Revision,
    Diff {
        comparison: ReviewComparison,
        instance: usize,
    },
}

impl DocumentKind {
    pub fn suffix(self) -> Cow<'static, str> {
        match self {
            Self::Diff {
                comparison,
                instance,
            } => {
                let number = if instance > 1 {
                    format!(" #{instance}")
                } else {
                    String::new()
                };
                format!(" [diff: {}{number}]", comparison.name()).into()
            }
            _ => "".into(),
        }
    }
}

/// A source path is presentation metadata; it is never a save destination or URI.
#[derive(Debug, Clone, Copy)]
pub struct DocumentDisplay<'a> {
    pub source_path: Option<&'a Path>,
    pub kind: DocumentKind,
    pub revision: Option<&'a str>,
}

impl DocumentDisplay<'_> {
    pub fn format_name(self, path: &Path) -> String {
        if let Some(label) = self.revision {
            let kind = if matches!(self.kind, DocumentKind::Diff { .. }) {
                "diff: "
            } else {
                ""
            };
            let instance = match self.kind {
                DocumentKind::Diff { instance, .. } if instance > 1 => format!(" #{instance}"),
                _ => String::new(),
            };
            format!("{} [{kind}{label}{instance}]", self.format_path(path))
        } else {
            format!("{}{}", self.format_path(path), self.kind.suffix())
        }
    }

    pub fn format_path(self, path: &Path) -> String {
        let label = path.to_string_lossy();
        if matches!(
            self.kind,
            DocumentKind::Diff { .. } | DocumentKind::Revision
        ) {
            label
                .chars()
                .map(|ch| {
                    if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') {
                        '�'
                    } else {
                        ch
                    }
                })
                .collect()
        } else {
            label.into_owned()
        }
    }
}

impl Document {
    pub fn display_metadata(&self) -> DocumentDisplay<'_> {
        if let Some(review) = self.review() {
            return DocumentDisplay {
                source_path: Some(review.path()),
                revision: review.revision_label(),
                kind: DocumentKind::Diff {
                    comparison: review.comparison(),
                    instance: review.instance(),
                },
            };
        }
        if let Some(source) = &self.historical {
            return DocumentDisplay {
                source_path: Some(&source.path),
                kind: DocumentKind::Revision,
                revision: Some(&source.label),
            };
        }
        DocumentDisplay {
            source_path: self.path(),
            revision: None,
            kind: if self.is_virtual() {
                DocumentKind::Virtual
            } else if self.path().is_some() {
                DocumentKind::File
            } else {
                DocumentKind::Scratch
            },
        }
    }
}

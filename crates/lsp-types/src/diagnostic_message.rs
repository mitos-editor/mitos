use serde::{Deserialize, Serialize};

use crate::MarkupContent;

/// A diagnostic message. Markup is supported since LSP 3.18 and requires
/// `textDocument.diagnostic.markupMessageSupport`.
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DiagnosticMessage {
    String(String),
    MarkupContent(MarkupContent),
}

impl DiagnosticMessage {
    /// The text of the message, preserving markup when present.
    pub fn as_str(&self) -> &str {
        match self {
            Self::String(text) => text,
            Self::MarkupContent(content) => &content.value,
        }
    }
}

impl Default for DiagnosticMessage {
    fn default() -> Self {
        Self::String(String::new())
    }
}

impl From<String> for DiagnosticMessage {
    fn from(text: String) -> Self {
        Self::String(text)
    }
}

impl From<&str> for DiagnosticMessage {
    fn from(text: &str) -> Self {
        Self::String(text.to_owned())
    }
}

impl From<MarkupContent> for DiagnosticMessage {
    fn from(content: MarkupContent) -> Self {
        Self::MarkupContent(content)
    }
}

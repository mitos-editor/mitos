//! Server-provided document content, standardized in LSP 3.18.
//!
//! <https://microsoft.github.io/language-server-protocol/specifications/lsp/3.18/specification/#textDocumentContent>

use serde::{Deserialize, Serialize};

use crate::{StaticRegistrationOptions, Uri};

/// Client capabilities for `workspace/textDocumentContent`.
#[derive(Debug, Eq, PartialEq, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentContentClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_registration: Option<bool>,
}

/// URI schemes for which the server provides document content.
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct TextDocumentContentOptions {
    pub schemes: Vec<String>,
}

/// Static or dynamic registration of a document content provider.
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct TextDocumentContentRegistrationOptions {
    #[serde(flatten)]
    pub text_document_content_options: TextDocumentContentOptions,

    #[serde(flatten)]
    pub static_registration_options: StaticRegistrationOptions,
}

/// Parameters of `workspace/textDocumentContent`.
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct TextDocumentContentParams {
    pub uri: Uri,
}

/// The document content. Clients may normalize whitespace and line endings
/// before sending subsequent document open notifications.
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct TextDocumentContentResult {
    pub text: String,
}

/// Parameters of `workspace/textDocumentContent/refresh`.
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct TextDocumentContentRefreshParams {
    pub uri: Uri,
}

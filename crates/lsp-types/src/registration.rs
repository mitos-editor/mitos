//! Registration types whose wire layouts reuse existing provider options.

use serde::{Deserialize, Serialize};

use crate::{
    CodeActionOptions, CodeLensOptions, ColorProviderOptions, DocumentLinkOptions,
    DocumentRangeFormattingOptions, DocumentSymbolOptions, FoldingProviderOptions, GenericOptions,
    GenericRegistrationOptions, OneOf, ReferencesOptions, RenameOptions,
    StaticTextDocumentColorProviderOptions, TextDocumentRegistrationOptions,
    WorkspaceSymbolOptions,
};

pub type ImplementationOptions = GenericOptions;
pub type TypeDefinitionOptions = GenericOptions;
pub type DocumentColorOptions = ColorProviderOptions;
pub type FoldingRangeOptions = FoldingProviderOptions;
pub type ReferenceOptions = ReferencesOptions;
pub type ImplementationRegistrationOptions = GenericRegistrationOptions;
pub type TypeDefinitionRegistrationOptions = GenericRegistrationOptions;
pub type DocumentColorRegistrationOptions = StaticTextDocumentColorProviderOptions;
pub type FoldingRangeRegistrationOptions = StaticTextDocumentColorProviderOptions;
pub type CallHierarchyRegistrationOptions = GenericRegistrationOptions;
pub type DefinitionRegistrationOptions = GenericRegistrationOptions;
pub type ReferenceRegistrationOptions = GenericRegistrationOptions;
pub type DocumentHighlightRegistrationOptions = GenericRegistrationOptions;
pub type DocumentFormattingRegistrationOptions = GenericRegistrationOptions;
pub type WorkspaceSymbolRegistrationOptions = WorkspaceSymbolOptions;

/// Dynamic registration options for the DocumentSymbol request.
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct DocumentSymbolRegistrationOptions {
    #[serde(flatten)]
    pub text_document_registration_options: TextDocumentRegistrationOptions,

    #[serde(flatten)]
    pub document_symbol_options: DocumentSymbolOptions,
}

/// Dynamic registration options for the CodeAction request.
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct CodeActionRegistrationOptions {
    #[serde(flatten)]
    pub text_document_registration_options: TextDocumentRegistrationOptions,

    #[serde(flatten)]
    pub code_action_options: CodeActionOptions,
}

/// Dynamic registration options for the CodeLens request.
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct CodeLensRegistrationOptions {
    #[serde(flatten)]
    pub text_document_registration_options: TextDocumentRegistrationOptions,

    #[serde(flatten)]
    pub code_lens_options: CodeLensOptions,
}

/// Dynamic registration options for the DocumentLink request.
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct DocumentLinkRegistrationOptions {
    #[serde(flatten)]
    pub text_document_registration_options: TextDocumentRegistrationOptions,

    #[serde(flatten)]
    pub document_link_options: DocumentLinkOptions,
}

/// Dynamic registration options for the DocumentRangeFormatting request.
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct DocumentRangeFormattingRegistrationOptions {
    #[serde(flatten)]
    pub text_document_registration_options: TextDocumentRegistrationOptions,

    #[serde(flatten)]
    pub document_range_formatting_options: DocumentRangeFormattingOptions,
}

/// Dynamic registration options for the Rename request.
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct RenameRegistrationOptions {
    #[serde(flatten)]
    pub text_document_registration_options: TextDocumentRegistrationOptions,

    #[serde(flatten)]
    pub rename_options: RenameOptions,
}

/// Configuration sections included in a dynamic registration.
#[derive(Debug, Eq, PartialEq, Clone, Default, Deserialize, Serialize)]
pub struct DidChangeConfigurationRegistrationOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<OneOf<String, Vec<String>>>,
}

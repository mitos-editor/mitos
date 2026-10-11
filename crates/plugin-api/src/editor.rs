//! Closed, owned editor services. Caller ownership is supplied by the host.
//!
//! A service is bound to one plugin and host generation; requests cannot name a
//! different owner. Hosts check permissions, live handles and revisions again
//! when applying asynchronous results. Syntax and language replies contain only
//! owned values, never native nodes, clients, pointers or borrowed editor state.
//!
//! | Operation | Required capabilities |
//! | --- | --- |
//! | Catalog, syntax, language, settings or register read | `EditorRead` |
//! | Publish or clear owned document diagnostics | `EditorRead`, `Ui` |
//! | Scratch, focus, split, close | `EditorNavigate` |
//! | Ordinary register write | `EditorSelection` |
//! | Clipboard register read/write | Above capability and `Clipboard` |
//! | Setting override/clear | `EditorSettings` |
//!
//! Transport adapters bound each serialized request to 4 KiB and each reply to
//! 1 MiB. Native adapters also enforce the operation limits below before doing
//! work. Language requests have a host deadline, and completion checks the
//! original document version; a successful stale reply is never published.
//! Syntax queries initially support structural captures only: predicates return
//! `UnsupportedInterface`. The adapter bounds query source to 4 KiB, document
//! bytes to 16 MiB and queried range to 1 MiB. Its native cursor has no progress
//! callback, so the two-second caller deadline cannot interrupt an already
//! running compile/cursor operation. One editor-wide work permit remains held
//! until that native operation actually finishes, including after timeout.

use serde::{Deserialize, Serialize};

use crate::{Capability, ErrorCode, ServiceError};

pub const MAX_EDITOR_REQUEST_BYTES: usize = 4 * 1024;
pub const MAX_EDITOR_REPLY_BYTES: usize = 1024 * 1024;
pub const MAX_SYNTAX_CAPTURES: u32 = 256;
pub const MAX_LANGUAGE_SYMBOLS: u32 = 256;
pub const MAX_LANGUAGE_CODE_ACTIONS: usize = 64;
pub const MAX_LANGUAGE_CODE_ACTION_EDITS: usize = 256;
pub const MAX_LANGUAGE_CODE_ACTION_KINDS: usize = 16;
pub const MAX_UNSAVED_DOCUMENTS: u32 = 128;
pub const MAX_DOCUMENT_DIAGNOSTICS: usize = 32;
pub const MAX_DOCUMENT_DIAGNOSTIC_MESSAGE_BYTES: usize = 512;
pub const MAX_REGISTER_VALUES: usize = 64;
pub const MAX_REGISTER_BYTES: usize = 4 * 1024;
pub const LANGUAGE_DEADLINE_MILLIS: u64 = 2_000;

/// A session-local document and the text version captured by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentTarget {
    pub document: u64,
    pub version: i32,
}

/// An explicit view binding and selection. Services never infer this from focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewTarget {
    pub view: u64,
    pub document: u64,
    pub binding_revision: u64,
    pub version: i32,
    pub selection_revision: u64,
}

/// Half-open Unicode scalar offsets, independent of UTF-8 byte representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

/// One native diagnostic annotation at the request's document version.
/// Ownership is supplied by the host; publishing replaces only this plugin's
/// annotations for that document. The host checks scalar ranges against text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentDiagnostic {
    pub range: TextRange,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenDisposition {
    Replace,
    HorizontalSplit,
    VerticalSplit,
}

/// A bounded native operation; this is not an arbitrary method/RPC namespace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EditorRequest {
    /// Query bounded current document/view metadata without an event snapshot.
    /// Catalog cursors and page limits follow the existing state-query contract.
    ReadState {
        query: crate::StateQuery,
    },
    /// Replace this plugin's annotations for the target document and version.
    /// Native LSP diagnostics and other plugins' annotations are preserved.
    PublishDiagnostics {
        target: DocumentTarget,
        diagnostics: Vec<DocumentDiagnostic>,
    },
    /// Clear this plugin's annotations, either for one current document/version
    /// or, with no target, across all open documents.
    ClearDiagnostics {
        #[serde(default)]
        target: Option<DocumentTarget>,
    },
    /// Whether the captured buffer differs from its last successful save.
    /// Disk-based tools must not assume that an editor snapshot was saved.
    DocumentStatus {
        target: DocumentTarget,
    },
    /// List current modified buffers before invoking a disk-based project tool.
    /// A truncated list cannot prove that the rest of the project is saved.
    UnsavedDocuments {
        /// Zero chooses 128. Paths are metadata of already-open documents.
        #[serde(default)]
        max_documents: u32,
    },
    SyntaxQuery {
        target: DocumentTarget,
        range: TextRange,
        /// A structural query compiled for the target's existing root grammar.
        /// The initial adapter rejects predicates with `UnsupportedInterface`.
        query: String,
        /// Zero chooses 256. Injection grammars are not guessed or selected by
        /// an unvalidated foreign grammar handle.
        #[serde(default)]
        max_captures: u32,
    },
    LanguageHover {
        target: DocumentTarget,
        offset: u64,
        /// An attached server name, not an executable or a provider handle.
        #[serde(default)]
        server: Option<String>,
    },
    LanguageSymbols {
        target: DocumentTarget,
        #[serde(default)]
        server: Option<String>,
        #[serde(default)]
        max_symbols: u32,
    },
    /// Request edits from an already attached formatting server. This does not
    /// execute a configured external formatter or apply the returned edits.
    LanguageFormat {
        target: DocumentTarget,
        #[serde(default)]
        server: Option<String>,
    },
    /// Query an already attached code-action server. Returned actions contain
    /// only edits for this document at this version, never executable commands.
    /// Lazy data-only actions may be resolved within the same host deadline.
    /// Source documents are limited to 1 MiB to bound native position conversion.
    /// Applying a returned action separately requires `EditorEdit`.
    LanguageCodeActions {
        target: DocumentTarget,
        range: TextRange,
        /// Hierarchical LSP kinds, for example `source.organizeImports` or
        /// `refactor.rewrite`. An empty list requests all available kinds.
        #[serde(default)]
        kinds: Vec<String>,
        #[serde(default)]
        server: Option<String>,
    },
    /// Scoped text-file navigation. Coordinates count lines and Unicode scalar
    /// columns from zero; invalid coordinates are rejected rather than clamped.
    OpenAt {
        origin: Option<ViewTarget>,
        path: String,
        line: u64,
        column: u64,
        action: OpenDisposition,
    },
    Scratch {
        name: String,
        text: String,
        #[serde(default)]
        language: Option<String>,
        /// Omitted only when the editor has no views. Creation never overwrites
        /// an existing document, including another owner's named scratch.
        origin: Option<ViewTarget>,
        action: OpenDisposition,
    },
    Focus {
        target: ViewTarget,
    },
    Split {
        target: ViewTarget,
        action: OpenDisposition,
    },
    CloseView {
        target: ViewTarget,
    },
    /// Modified documents are rejected; no implicit discard or save.
    CloseDocument {
        target: DocumentTarget,
    },
    ReadRegister {
        name: char,
        /// Required for selection-derived registers. The target must be current;
        /// ordinary and clipboard registers need no focused view.
        #[serde(default)]
        origin: Option<ViewTarget>,
    },
    WriteRegister {
        name: char,
        values: Vec<String>,
    },
    ReadSettings {
        scope: SettingsScope,
    },
    OverrideSetting {
        scope: SettingsScope,
        value: SettingValue,
    },
    ClearSettings {
        scope: SettingsScope,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SettingsScope {
    Editor,
    Document { target: DocumentTarget },
}

/// Deliberately narrow settings; process, filesystem, provider and shell policy
/// are not settings a plugin can change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum SettingValue {
    AutoFormat(bool),
    SoftWrap(bool),
    CursorLine(bool),
    /// Editor scope only; a theme identifier without directory separators.
    Theme(String),
}

/// One owned query capture; no node identity remains after this reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntaxCapture {
    pub name: String,
    pub range: TextRange,
}

/// Flattened native symbol information. Parent indices refer only to this reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: String,
    pub range: TextRange,
    pub selection: TextRange,
    pub parent: Option<u32>,
}

/// An actionable, owned patch to the reply's original document and version.
/// Command-bearing, disabled, multi-document and file-operation actions are
/// omitted. No opaque LSP data or executable command crosses this boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageCodeAction {
    pub title: String,
    pub kind: Option<String>,
    pub edits: Vec<crate::TextEdit>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnsavedDocument {
    pub target: DocumentTarget,
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EditorReply {
    State {
        catalog: crate::StateCatalog,
    },
    DocumentStatus {
        target: DocumentTarget,
        modified: bool,
    },
    UnsavedDocuments {
        documents: Vec<UnsavedDocument>,
        truncated: bool,
    },
    Syntax {
        target: DocumentTarget,
        captures: Vec<SyntaxCapture>,
        truncated: bool,
    },
    Hover {
        target: DocumentTarget,
        offset: u64,
        markdown: String,
        range: Option<TextRange>,
    },
    Symbols {
        target: DocumentTarget,
        symbols: Vec<LanguageSymbol>,
        truncated: bool,
    },
    Edits {
        target: DocumentTarget,
        edits: Vec<crate::TextEdit>,
    },
    CodeActions {
        target: DocumentTarget,
        actions: Vec<LanguageCodeAction>,
        /// True when actions were omitted as unsupported or to preserve
        /// bounds. An empty, truncated reply is not a usable action.
        truncated: bool,
    },
    View {
        target: ViewTarget,
    },
    Closed {
        document: Option<u64>,
        view: Option<u64>,
    },
    Register {
        values: Vec<String>,
    },
    Settings {
        values: Vec<SettingValue>,
    },
    Updated,
}

impl EditorRequest {
    /// Native authorization still checks declaration AND live user grants.
    pub fn capabilities(&self) -> &'static [Capability] {
        match self {
            Self::PublishDiagnostics { .. } | Self::ClearDiagnostics { .. } => {
                &[Capability::EditorRead, Capability::Ui]
            }
            Self::ReadRegister {
                name: '*' | '+', ..
            } => &[Capability::EditorRead, Capability::Clipboard],
            Self::WriteRegister {
                name: '*' | '+', ..
            } => &[Capability::EditorSelection, Capability::Clipboard],
            Self::ReadState { .. }
            | Self::DocumentStatus { .. }
            | Self::UnsavedDocuments { .. }
            | Self::SyntaxQuery { .. }
            | Self::LanguageHover { .. }
            | Self::LanguageSymbols { .. }
            | Self::LanguageFormat { .. }
            | Self::LanguageCodeActions { .. }
            | Self::ReadRegister { .. }
            | Self::ReadSettings { .. } => &[Capability::EditorRead],
            Self::OpenAt { .. } => &[
                Capability::EditorNavigate,
                Capability::EditorSelection,
                Capability::WorkspaceRead,
            ],
            Self::Scratch { .. }
            | Self::Focus { .. }
            | Self::Split { .. }
            | Self::CloseView { .. }
            | Self::CloseDocument { .. } => &[Capability::EditorNavigate],
            Self::WriteRegister { .. } => &[Capability::EditorSelection],
            Self::OverrideSetting { .. } | Self::ClearSettings { .. } => {
                &[Capability::EditorSettings]
            }
        }
    }

    /// Check operation bounds before grammar compilation or native dispatch.
    /// Transport and adapter checks additionally validate total encoded size,
    /// document lengths, revisions, server attachment and owner liveness.
    pub fn validate(&self) -> Result<(), ServiceError> {
        match self {
            Self::ReadState { query } if query.limit > 64 => {
                return Err(exhausted("state catalog page limit exceeds 64"));
            }
            Self::PublishDiagnostics { diagnostics, .. } => {
                if diagnostics.len() > MAX_DOCUMENT_DIAGNOSTICS {
                    return Err(exhausted("document diagnostics exceed 32"));
                }
                for diagnostic in diagnostics {
                    if diagnostic.range.start > diagnostic.range.end {
                        return Err(invalid("document diagnostic needs an ordered range"));
                    }
                    if diagnostic.message.is_empty() {
                        return Err(invalid("document diagnostic needs a message"));
                    }
                    if diagnostic.message.len() > MAX_DOCUMENT_DIAGNOSTIC_MESSAGE_BYTES {
                        return Err(exhausted("document diagnostic message exceeds 512 bytes"));
                    }
                }
            }
            Self::UnsavedDocuments { max_documents } if *max_documents > MAX_UNSAVED_DOCUMENTS => {
                return Err(exhausted("unsaved document limit exceeds 128"));
            }
            Self::OpenAt { path, .. }
                if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) =>
            {
                return Err(invalid("open path must be nonempty bounded text"));
            }
            Self::SyntaxQuery {
                range,
                query,
                max_captures,
                ..
            } => {
                if range.start > range.end || query.is_empty() {
                    return Err(invalid("syntax query needs an ordered range and query"));
                }
                if *max_captures > MAX_SYNTAX_CAPTURES {
                    return Err(exhausted("syntax query capture limit exceeds 256"));
                }
            }
            Self::LanguageSymbols { max_symbols, .. } if *max_symbols > MAX_LANGUAGE_SYMBOLS => {
                return Err(exhausted("language symbol limit exceeds 256"));
            }
            Self::LanguageCodeActions { range, kinds, .. } => {
                if range.start > range.end {
                    return Err(invalid("code actions need an ordered range"));
                }
                if kinds.len() > MAX_LANGUAGE_CODE_ACTION_KINDS {
                    return Err(exhausted("code action kinds exceed 16"));
                }
                if kinds.iter().any(|kind| {
                    kind.is_empty() || kind.len() > 128 || kind.chars().any(char::is_control)
                }) {
                    return Err(invalid("code action kinds must be nonempty bounded text"));
                }
            }
            Self::Scratch { name, .. }
                if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) =>
            {
                return Err(invalid("scratch name must be nonempty bounded text"));
            }
            Self::Split {
                action: OpenDisposition::Replace,
                ..
            } => {
                return Err(invalid("split requires horizontal-split or vertical-split"));
            }
            Self::ReadRegister {
                name: '#' | '.' | '%',
                origin: None,
            } => {
                return Err(invalid("derived register requires an originating view"));
            }
            Self::WriteRegister {
                name: '#' | '.' | '%',
                ..
            } => {
                return Err(invalid("derived register is read-only"));
            }
            Self::WriteRegister { values, .. } => {
                let bytes = values
                    .iter()
                    .try_fold(0usize, |total, value| total.checked_add(value.len()));
                if values.len() > MAX_REGISTER_VALUES
                    || bytes.is_none_or(|bytes| bytes > MAX_REGISTER_BYTES)
                {
                    return Err(exhausted("register contents exceed the service limit"));
                }
            }
            Self::OverrideSetting {
                scope: SettingsScope::Document { .. },
                value: SettingValue::Theme(_),
            } => {
                return Err(invalid("theme override requires editor scope"));
            }
            Self::OverrideSetting {
                value: SettingValue::Theme(name),
                ..
            } if name.is_empty()
                || name.len() > 128
                || matches!(name.as_str(), "." | "..")
                || name
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':')) =>
            {
                return Err(invalid("theme name must be a bounded theme identifier"));
            }
            _ => (),
        }
        Ok(())
    }
}

fn invalid(message: &str) -> ServiceError {
    ServiceError::new(ErrorCode::InvalidRequest, message)
}

fn exhausted(message: &str) -> ServiceError {
    ServiceError::new(ErrorCode::ResourceExhausted, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_annotations_have_bounded_content_and_explicit_authority() {
        let target = DocumentTarget {
            document: 1,
            version: 7,
        };
        let diagnostic = DocumentDiagnostic {
            range: TextRange { start: 1, end: 2 },
            severity: DiagnosticSeverity::Error,
            message: "test assertion failed".into(),
        };
        let publish = |diagnostics| EditorRequest::PublishDiagnostics {
            target,
            diagnostics,
        };
        let request = publish(vec![diagnostic.clone(); MAX_DOCUMENT_DIAGNOSTICS]);
        request.validate().unwrap();
        assert_eq!(
            request.capabilities(),
            &[Capability::EditorRead, Capability::Ui]
        );
        for target in [Some(target), None] {
            let clear = EditorRequest::ClearDiagnostics { target };
            clear.validate().unwrap();
            assert_eq!(clear.capabilities(), request.capabilities());
        }
        assert_eq!(
            publish(vec![diagnostic.clone(); MAX_DOCUMENT_DIAGNOSTICS + 1])
                .validate()
                .unwrap_err()
                .code,
            ErrorCode::ResourceExhausted
        );
        for (range, message, expected) in [
            (
                TextRange { start: 2, end: 1 },
                diagnostic.message.clone(),
                ErrorCode::InvalidRequest,
            ),
            (diagnostic.range, String::new(), ErrorCode::InvalidRequest),
            (
                diagnostic.range,
                "μ".repeat(MAX_DOCUMENT_DIAGNOSTIC_MESSAGE_BYTES / 2 + 1),
                ErrorCode::ResourceExhausted,
            ),
        ] {
            assert_eq!(
                publish(vec![DocumentDiagnostic {
                    range,
                    message,
                    ..diagnostic.clone()
                }])
                .validate()
                .unwrap_err()
                .code,
                expected
            );
        }
        publish(vec![DocumentDiagnostic {
            range: TextRange { start: 2, end: 2 },
            message: "μ".repeat(MAX_DOCUMENT_DIAGNOSTIC_MESSAGE_BYTES / 2),
            ..diagnostic
        }])
        .validate()
        .unwrap();
        assert!(
            serde_json::from_value::<DocumentDiagnostic>(serde_json::json!({
                "range": {"start": 1, "end": 2},
                "severity": "error",
                "message": "test failed",
                "owner": "another-plugin"
            }))
            .is_err()
        );
    }

    #[test]
    fn immediate_state_queries_require_reads_and_bounded_pages() {
        for limit in [0, 64] {
            let request = EditorRequest::ReadState {
                query: crate::StateQuery {
                    limit,
                    ..Default::default()
                },
            };
            assert_eq!(request.capabilities(), &[Capability::EditorRead]);
            request.validate().unwrap();
        }
        assert_eq!(
            EditorRequest::ReadState {
                query: crate::StateQuery {
                    limit: 65,
                    ..Default::default()
                },
            }
            .validate()
            .unwrap_err()
            .code,
            ErrorCode::ResourceExhausted
        );
    }

    #[test]
    fn clipboard_and_settings_authority_are_explicit() {
        let clipboard = EditorRequest::WriteRegister {
            name: '+',
            values: vec!["copied".into()],
        };
        assert_eq!(
            clipboard.capabilities(),
            &[Capability::EditorSelection, Capability::Clipboard]
        );
        let settings = EditorRequest::OverrideSetting {
            scope: SettingsScope::Editor,
            value: SettingValue::AutoFormat(false),
        };
        assert_eq!(settings.capabilities(), &[Capability::EditorSettings]);
        assert!(serde_json::from_str::<EditorRequest>(r#"{"kind":"override-setting","scope":{"kind":"editor"},"value":{"kind":"shell","value":"sh"}}"#).is_err());
    }

    #[test]
    fn queries_and_registers_reject_unbounded_or_ambiguous_work() {
        let query = EditorRequest::SyntaxQuery {
            target: DocumentTarget {
                document: 1,
                version: 0,
            },
            range: TextRange { start: 0, end: 10 },
            query: "(identifier) @name".into(),
            max_captures: 257,
        };
        assert_eq!(
            query.validate().unwrap_err().code,
            ErrorCode::ResourceExhausted
        );
        let register = EditorRequest::WriteRegister {
            name: 'a',
            values: vec!["x".repeat(MAX_REGISTER_BYTES + 1)],
        };
        assert_eq!(
            register.validate().unwrap_err().code,
            ErrorCode::ResourceExhausted
        );
        let derived = EditorRequest::ReadRegister {
            name: '.',
            origin: None,
        };
        assert_eq!(
            derived.validate().unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        assert!(serde_json::from_str::<EditorRequest>(
            r#"{"kind":"read-register","name":"a","owner":"another-plugin"}"#
        )
        .is_err());
    }

    #[test]
    fn theme_overrides_accept_identifiers_without_path_authority() {
        for name in [
            "../secret",
            "/tmp/theme",
            "..",
            ".",
            "C:\\theme",
            "dir\\theme",
        ] {
            let request = EditorRequest::OverrideSetting {
                scope: SettingsScope::Editor,
                value: SettingValue::Theme(name.into()),
            };
            assert_eq!(
                request.validate().unwrap_err().code,
                ErrorCode::InvalidRequest
            );
        }
        for name in ["default", "base16_default", "owner.theme"] {
            EditorRequest::OverrideSetting {
                scope: SettingsScope::Editor,
                value: SettingValue::Theme(name.into()),
            }
            .validate()
            .unwrap();
        }
    }

    #[test]
    fn code_actions_are_readonly_bounded_requests_without_command_results() {
        let mut request = EditorRequest::LanguageCodeActions {
            target: DocumentTarget {
                document: 1,
                version: 7,
            },
            range: TextRange { start: 1, end: 2 },
            kinds: vec!["source.organizeImports".into()],
            server: Some("gopls".into()),
        };
        assert_eq!(request.capabilities(), &[Capability::EditorRead]);
        request.validate().unwrap();
        assert_eq!(
            serde_json::from_value::<EditorRequest>(serde_json::to_value(&request).unwrap())
                .unwrap(),
            request
        );
        let EditorRequest::LanguageCodeActions { range, .. } = &mut request else {
            unreachable!()
        };
        range.start = 3;
        assert_eq!(
            request.validate().unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        let EditorRequest::LanguageCodeActions { range, kinds, .. } = &mut request else {
            unreachable!()
        };
        range.start = 1;
        *kinds = vec!["refactor".into(); MAX_LANGUAGE_CODE_ACTION_KINDS + 1];
        assert_eq!(
            request.validate().unwrap_err().code,
            ErrorCode::ResourceExhausted
        );
        assert!(
            serde_json::from_value::<LanguageCodeAction>(serde_json::json!({
                "title": "Unsafe", "kind": null, "edits": [], "command": "gopls.apply_fix"
            }))
            .is_err()
        );
    }

    #[test]
    fn unsaved_catalog_is_readonly_bounded_and_reports_partial_information() {
        let request = EditorRequest::UnsavedDocuments { max_documents: 0 };
        assert_eq!(request.capabilities(), &[Capability::EditorRead]);
        request.validate().unwrap();
        assert_eq!(
            EditorRequest::UnsavedDocuments { max_documents: 129 }
                .validate()
                .unwrap_err()
                .code,
            ErrorCode::ResourceExhausted
        );
        let reply = EditorReply::UnsavedDocuments {
            documents: vec![UnsavedDocument {
                target: DocumentTarget {
                    document: 1,
                    version: 7,
                },
                path: None,
            }],
            truncated: true,
        };
        assert_eq!(
            serde_json::from_value::<EditorReply>(serde_json::to_value(&reply).unwrap()).unwrap(),
            reply
        );
    }
}

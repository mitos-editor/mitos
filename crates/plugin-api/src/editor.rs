//! Closed, owned editor services. Caller ownership is supplied by the host.
//!
//! A service is bound to one plugin and host generation; requests cannot name a
//! different owner. Hosts check permissions, live handles and revisions again
//! when applying asynchronous results. Syntax and language replies contain only
//! owned values, never native nodes, clients, pointers or borrowed editor state.
//!
//! | Operation | Required capabilities |
//! | --- | --- |
//! | Syntax, language, settings or register read | `EditorRead` |
//! | Scratch, focus, split, close | `EditorNavigate` |
//! | Ordinary register write | `EditorSelection` |
//! | Clipboard register read/write | Above capability and `Clipboard` |
//! | Setting override/clear | `EditorSettings` |
//!
//! Transport adapters bound each serialized request to 4 KiB and each reply to
//! 1 MiB. Native adapters also enforce the operation limits below before doing
//! work. Language requests have a host deadline, and completion checks the
//! original document version; a successful stale reply is never published.

use serde::{Deserialize, Serialize};

use crate::{Capability, ErrorCode, ServiceError};

pub const MAX_EDITOR_REQUEST_BYTES: usize = 4 * 1024;
pub const MAX_EDITOR_REPLY_BYTES: usize = 1024 * 1024;
pub const MAX_SYNTAX_CAPTURES: u32 = 256;
pub const MAX_LANGUAGE_SYMBOLS: u32 = 256;
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
    SyntaxQuery {
        target: DocumentTarget,
        range: TextRange,
        /// A query compiled for the target's existing root grammar. Predicates
        /// are limited to the native adapter's supported, bounded predicates.
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
    /// Editor scope only; the existing theme loader validates the name.
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EditorReply {
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
    pub fn capabilities(&self) -> Vec<Capability> {
        let mut required = match self {
            Self::SyntaxQuery { .. }
            | Self::LanguageHover { .. }
            | Self::LanguageSymbols { .. }
            | Self::ReadRegister { .. }
            | Self::ReadSettings { .. } => vec![Capability::EditorRead],
            Self::Scratch { .. }
            | Self::Focus { .. }
            | Self::Split { .. }
            | Self::CloseView { .. }
            | Self::CloseDocument { .. } => vec![Capability::EditorNavigate],
            Self::WriteRegister { .. } => vec![Capability::EditorSelection],
            Self::OverrideSetting { .. } | Self::ClearSettings { .. } => {
                vec![Capability::EditorSettings]
            }
        };
        if matches!(
            self,
            Self::ReadRegister {
                name: '*' | '+',
                ..
            } | Self::WriteRegister {
                name: '*' | '+',
                ..
            }
        ) {
            required.push(Capability::Clipboard);
        }
        required
    }

    /// Check operation bounds before grammar compilation or native dispatch.
    /// Transport and adapter checks additionally validate total encoded size,
    /// document lengths, revisions, server attachment and owner liveness.
    pub fn validate(&self) -> Result<(), ServiceError> {
        match self {
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
            } if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) => {
                return Err(invalid("theme name must be nonempty bounded text"));
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
    fn clipboard_and_settings_authority_are_explicit() {
        let clipboard = EditorRequest::WriteRegister {
            name: '+',
            values: vec!["copied".into()],
        };
        assert_eq!(
            clipboard.capabilities(),
            vec![Capability::EditorSelection, Capability::Clipboard]
        );
        let settings = EditorRequest::OverrideSetting {
            scope: SettingsScope::Editor,
            value: SettingValue::AutoFormat(false),
        };
        assert_eq!(settings.capabilities(), vec![Capability::EditorSettings]);
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
}

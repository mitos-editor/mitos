//! Owned terminal requests. Guests supply models; the host supplies ownership.
//!
//! Rendering and input processing use cached native components. No operation in
//! this module requires calling a guest while drawing an editor frame.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{Capability, ErrorCode, ServiceError};

pub const MAX_UI_TITLE_BYTES: usize = 512;
pub const MAX_UI_INPUT_BYTES: usize = 16 * 1024;
pub const MAX_UI_ROWS: usize = 1024;
pub const MAX_UI_ROW_BYTES: usize = 4096;
pub const MAX_UI_PREVIEW_BYTES: usize = 64 * 1024;
pub const MAX_UI_MODEL_BYTES: usize = 1024 * 1024;
pub const MAX_PLUGIN_KEYBINDINGS: usize = 64;
pub const MAX_BUILTIN_COMMANDS: usize = 32;
pub const MAX_BUILTIN_COUNT: usize = 1024;
pub const MAX_NEXT_KEY_TIMEOUT_MS: u32 = 60_000;

/// Assigned by the host. A guest cannot choose another plugin's identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiOwner {
    pub plugin: String,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiIdentity {
    pub owner: UiOwner,
    /// Guest correlation ID; unique among this owner's outstanding requests.
    pub request: u64,
    /// Fresh host-issued instance token. Reusing a guest correlation ID must
    /// never revive an earlier dialog or allow its queued timeout to cancel it.
    pub token: u64,
}

/// The view binding at submission. Closing or rebinding it cancels the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiOrigin {
    pub view: u64,
    pub document: u64,
    pub binding_revision: u64,
    /// UI liveness follows the binding. Editing/composition additionally checks
    /// these revisions before acting on the originating selection.
    pub version: i32,
    pub selection_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiRequest {
    pub identity: UiIdentity,
    pub origin: Option<UiOrigin>,
    pub kind: UiKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum UiKind {
    Prompt {
        title: String,
        #[serde(default)]
        initial: String,
    },
    Picker {
        title: String,
        rows: Vec<UiRow>,
    },
    /// Capture one native key without interpreting it as editor commands.
    NextKey {
        title: String,
        timeout_ms: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiRow {
    /// Stable opaque ID returned on selection; it is not a command to evaluate.
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub preview: Option<String>,
    /// Existing document navigation; opening filesystem paths is a host service.
    #[serde(default)]
    pub location: Option<UiLocation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiLocation {
    pub document: u64,
    pub version: i32,
    /// Unicode scalar offset. The backend validates it before navigation.
    pub offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiOpenAction {
    Replace,
    HorizontalSplit,
    VerticalSplit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum UiValue {
    Prompt {
        text: String,
    },
    Picker {
        row: String,
        action: UiOpenAction,
    },
    /// Host-generated native key notation. This value is never replayed.
    NextKey {
        key: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiCancellation {
    User,
    OwnerUnloaded,
    OriginLost,
    Replaced,
    Closed,
    Shutdown,
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case", deny_unknown_fields)]
pub enum UiOutcome {
    Accepted { value: UiValue },
    Cancelled { reason: UiCancellation },
    Failed { error: ServiceError },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiResponse {
    pub identity: UiIdentity,
    pub outcome: UiOutcome,
}

impl UiKind {
    /// Enforce resource bounds before copying or normalizing display strings.
    pub fn normalize(mut self) -> Result<Self, ServiceError> {
        let mut total = 0;
        match &mut self {
            Self::Prompt { title, initial } => {
                bounded(title, MAX_UI_TITLE_BYTES, &mut total)?;
                bounded(initial, MAX_UI_INPUT_BYTES, &mut total)?;
                *title = terminal_text(title, false);
                *initial = terminal_text(initial, false);
            }
            Self::Picker { title, rows } => {
                bounded(title, MAX_UI_TITLE_BYTES, &mut total)?;
                *title = terminal_text(title, false);
                if rows.len() > MAX_UI_ROWS {
                    return Err(exhausted("plugin picker has too many rows"));
                }
                let mut ids = BTreeSet::new();
                for row in rows {
                    bounded(&row.id, 128, &mut total)?;
                    if row.id.is_empty() || !ids.insert(row.id.clone()) {
                        return Err(ServiceError::new(
                            ErrorCode::InvalidRequest,
                            "plugin picker row IDs must be nonempty and unique",
                        ));
                    }
                    bounded(&row.label, MAX_UI_ROW_BYTES, &mut total)?;
                    bounded(&row.description, MAX_UI_ROW_BYTES, &mut total)?;
                    row.label = terminal_text(&row.label, false);
                    row.description = terminal_text(&row.description, false);
                    if let Some(preview) = &mut row.preview {
                        bounded(preview, MAX_UI_PREVIEW_BYTES, &mut total)?;
                        *preview = terminal_text(preview, true);
                    }
                }
            }
            Self::NextKey { title, timeout_ms } => {
                bounded(title, MAX_UI_TITLE_BYTES, &mut total)?;
                *title = terminal_text(title, false);
                if *timeout_ms == 0 || *timeout_ms > MAX_NEXT_KEY_TIMEOUT_MS {
                    return Err(ServiceError::new(
                        ErrorCode::InvalidRequest,
                        "next-key timeout must be between 1 and 60000 milliseconds",
                    ));
                }
            }
        }
        Ok(self)
    }
}

/// Strip terminal controls, retaining preview line breaks as text structure.
pub fn terminal_text(text: &str, multiline: bool) -> String {
    let mut output = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' if multiline => output.push('\n'),
            '\n' | '\r' | '\t' => output.push(' '),
            c if c.is_control() => (),
            c => output.push(c),
        }
    }
    output
}

fn bounded(text: &str, max: usize, total: &mut usize) -> Result<(), ServiceError> {
    *total = total.saturating_add(text.len());
    if text.len() > max || *total > MAX_UI_MODEL_BYTES {
        Err(exhausted("plugin UI model exceeds its size limit"))
    } else {
        Ok(())
    }
}

fn exhausted(message: &str) -> ServiceError {
    ServiceError::new(ErrorCode::ResourceExhausted, message)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeymapMode {
    Normal,
    Select,
    Insert,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginKeybinding {
    pub mode: KeymapMode,
    /// Native key notation, one key per element, with a maximum depth of eight.
    pub keys: Vec<String>,
    /// A declared local command belonging to the owning plugin; no arguments or
    /// expansion strings are evaluated during plugin keymap dispatch.
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeymapUpdate {
    pub identity: UiIdentity,
    pub bindings: Vec<PluginKeybinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeymapResponse {
    pub identity: UiIdentity,
    pub error: Option<ServiceError>,
}

/// Deliberately excludes typable commands, raw keys, shell execution, clipboard,
/// registers and process-launching commands. Extend it with reviewed authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuiltinCommand {
    MoveCharLeft,
    MoveCharRight,
    MoveLineUp,
    MoveLineDown,
    SelectAll,
    CollapseSelection,
    KeepPrimarySelection,
    DeleteSelectionNoYank,
    ChangeSelectionNoYank,
    Undo,
    Redo,
    InsertMode,
    NormalMode,
}

impl BuiltinCommand {
    pub fn name(self) -> &'static str {
        match self {
            Self::MoveCharLeft => "move_char_left",
            Self::MoveCharRight => "move_char_right",
            Self::MoveLineUp => "move_line_up",
            Self::MoveLineDown => "move_line_down",
            Self::SelectAll => "select_all",
            Self::CollapseSelection => "collapse_selection",
            Self::KeepPrimarySelection => "keep_primary_selection",
            Self::DeleteSelectionNoYank => "delete_selection_noyank",
            Self::ChangeSelectionNoYank => "change_selection_noyank",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::InsertMode => "insert_mode",
            Self::NormalMode => "normal_mode",
        }
    }

    pub fn capabilities(self) -> &'static [Capability] {
        match self {
            Self::DeleteSelectionNoYank | Self::Undo | Self::Redo => {
                &[Capability::EditorEdit, Capability::EditorSelection]
            }
            Self::ChangeSelectionNoYank => &[
                Capability::EditorEdit,
                Capability::EditorSelection,
                Capability::Ui,
            ],
            Self::InsertMode | Self::NormalMode => &[Capability::Ui, Capability::EditorSelection],
            _ => &[Capability::EditorSelection],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinInvocation {
    pub command: BuiltinCommand,
    #[serde(default)]
    pub count: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinRequest {
    pub identity: UiIdentity,
    pub origin: UiOrigin,
    pub commands: Vec<BuiltinInvocation>,
}

/// Composition preserves completed earlier commands when a later native
/// command fails. The count makes that partial outcome explicit to the guest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinResponse {
    pub identity: UiIdentity,
    pub completed: usize,
    pub error: Option<ServiceError>,
}

impl BuiltinRequest {
    pub fn validate(&self) -> Result<(), ServiceError> {
        if self.commands.len() > MAX_BUILTIN_COMMANDS
            || self.commands.iter().any(|command| {
                command
                    .count
                    .is_some_and(|count| count == 0 || count > MAX_BUILTIN_COUNT)
            })
        {
            return Err(exhausted("plugin builtin composition exceeds its limit"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_controls_are_removed_and_preview_line_breaks_preserved() {
        assert_eq!(
            terminal_text("μ\x1b[31m\x07\n\tX\u{009b}", false),
            "μ[31m  X"
        );
        assert_eq!(terminal_text("first\nsecond\x1b\t", true), "first\nsecond ");
    }

    #[test]
    fn picker_ids_and_aggregate_resources_are_bounded() {
        let row = UiRow {
            id: "same".into(),
            label: "row".into(),
            description: String::new(),
            preview: None,
            location: None,
        };
        let kind = UiKind::Picker {
            title: "pick".into(),
            rows: vec![row.clone(), row.clone()],
        };
        assert_eq!(
            kind.normalize().unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        let mut rows = (0..17)
            .map(|id| UiRow {
                id: id.to_string(),
                preview: Some("x".repeat(MAX_UI_PREVIEW_BYTES)),
                ..row.clone()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            UiKind::Picker {
                title: String::new(),
                rows: rows.clone()
            }
            .normalize()
            .unwrap_err()
            .code,
            ErrorCode::ResourceExhausted
        );
        rows.truncate(1);
        assert!(UiKind::Picker {
            title: String::new(),
            rows
        }
        .normalize()
        .is_ok());
    }

    #[test]
    fn unsafe_builtins_cannot_be_deserialized_into_composition() {
        for command in [
            "shell",
            "run-shell-command",
            "paste",
            "yank",
            ":open",
            "@iX<esc>",
        ] {
            assert!(serde_json::from_value::<BuiltinCommand>(serde_json::json!(command)).is_err());
        }
        assert_eq!(
            BuiltinCommand::DeleteSelectionNoYank.name(),
            "delete_selection_noyank"
        );
    }

    #[test]
    fn next_key_timeout_is_bounded_and_title_is_sanitized() {
        for timeout_ms in [0, MAX_NEXT_KEY_TIMEOUT_MS + 1] {
            assert_eq!(
                UiKind::NextKey {
                    title: "key".into(),
                    timeout_ms
                }
                .normalize()
                .unwrap_err()
                .code,
                ErrorCode::InvalidRequest
            );
        }
        assert_eq!(
            UiKind::NextKey {
                title: "μ\x1b\n".into(),
                timeout_ms: MAX_NEXT_KEY_TIMEOUT_MS
            }
            .normalize()
            .unwrap(),
            UiKind::NextKey {
                title: "μ ".into(),
                timeout_ms: MAX_NEXT_KEY_TIMEOUT_MS
            }
        );
    }
}

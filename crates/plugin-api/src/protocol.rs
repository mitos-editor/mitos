//! Runtime-neutral owned plugin protocol.
//!
//! Document offsets count Unicode scalar values, and view selection/binding
//! revisions are validated independently from document text versions.

use serde::{Deserialize, Serialize};

/// Experimental host protocol version, also used by the transitional memory ABI.
pub const ABI_VERSION: u32 = 3;

/// The invocation delivered to a plugin's request handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Event {
    Init,
    Shutdown,
    Command,
    DocumentOpened,
    DocumentChanged,
    DocumentSaved,
    DocumentClosed,
    SelectionChanged,
    ModeChanged,
    PostCommand,
    PostInsertChar,
    DocumentFocusLost,
    TerminalFocusGained,
    TerminalFocusLost,
    /// Events were lost to a bounded queue or causal limit. Query current state.
    ResyncRequired,
    /// A targeted response to Action::RequestState; data contains StateCatalog.
    State,
    /// Targeted native frontend completions, independent of subscriptions.
    UiResult,
    BuiltinResult,
    KeymapResult,
    /// A retained host job has buffered output or completion to drain.
    JobReady,
}

/// An owned snapshot of the editor and invocation arguments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub abi_version: u32,
    pub event: Event,
    /// The local command name declared in the plugin manifest.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// The plugin's configuration from the editor configuration file.
    #[serde(default)]
    pub config: serde_json::Value,
    pub editor: EditorContext,
    /// Additional event-specific metadata.
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EditorContext {
    /// The owning plugin-host generation; resources expire on reload/shutdown.
    pub generation: u64,
    pub mode: String,
    pub document: Option<DocumentSnapshot>,
    pub view: Option<ViewSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentSnapshot {
    /// A session-local document identifier; it is not a path or a persistent ID.
    pub id: u64,
    /// The revision against which an edit must be applied.
    pub version: i32,
    pub path: Option<String>,
    pub language: Option<String>,
    /// Text is read through an explicitly bounded region service.
    pub char_count: u64,
    pub byte_count: u64,
}

/// The originating view's binding and selection, independent of document text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSnapshot {
    /// An opaque generational handle, valid only in this editor generation.
    pub id: u64,
    pub document: u64,
    /// Changes whenever the view switches to another document, including away/back.
    pub binding_revision: u64,
    /// Changes independently of text versions whenever selection state changes.
    pub selection_revision: u64,
    pub selections: Vec<SelectionRange>,
    /// Index of the primary selection in `selections`.
    pub primary: usize,
}

/// A bounded current-state query. Catalog cursors are exclusive session handles.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateQuery {
    #[serde(default)]
    pub document: Option<u64>,
    #[serde(default)]
    pub view: Option<u64>,
    #[serde(default)]
    pub after_document: Option<u64>,
    #[serde(default)]
    pub after_view: Option<u64>,
    /// Zero chooses the default page size; the host caps every page at 64.
    #[serde(default)]
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentInfo {
    pub id: u64,
    pub version: i32,
    pub path: Option<String>,
    pub language: Option<String>,
    pub readonly: bool,
    pub binary: bool,
    pub bytes: usize,
    pub chars: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewInfo {
    pub id: u64,
    pub document: u64,
    pub binding_revision: u64,
    pub selection_revision: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateCatalog {
    pub documents: Vec<DocumentInfo>,
    pub views: Vec<ViewInfo>,
    pub next_document: Option<u64>,
    pub next_view: Option<u64>,
    /// An explicit query failure (closed target or oversized snapshot).
    pub error: Option<String>,
}

/// A selection's directed boundaries, measured in Unicode scalar values.
///
/// The selected text occupies `min(anchor, head)..max(anchor, head)`. Equal
/// boundaries describe an insertion cursor. The direction of the selection is
/// preserved by keeping the anchor and head distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionRange {
    pub anchor: usize,
    pub head: usize,
}

/// A replacement of the half-open scalar-value range `start..end`.
///
/// Ranges within one [`Action::Edit`] refer to the text before that action and
/// must not overlap. Later edit actions use the projected text after preceding
/// edits, while every action's version refers to the original snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// The actions to apply after a plugin invocation finishes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub error: Option<String>,
}

/// An editor operation permitted by the initial plugin API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Action {
    Edit {
        document: u64,
        /// The originating snapshot version, before any action in this response.
        version: i32,
        edits: Vec<TextEdit>,
    },
    /// Selection boundaries refer to the document after preceding edit actions.
    /// The version remains that of the originating snapshot, before those edits.
    SetSelection {
        document: u64,
        version: i32,
        view: u64,
        binding_revision: u64,
        selection_revision: u64,
        ranges: Vec<SelectionRange>,
        primary: usize,
    },
    Status {
        message: String,
    },
    Error {
        message: String,
    },
    Open {
        path: String,
    },
    RequestState {
        query: StateQuery,
    },
    ShowUi {
        request: u64,
        origin: Option<crate::ui::UiOrigin>,
        kind: crate::ui::UiKind,
    },
    InvokeBuiltin {
        request: u64,
        origin: crate::ui::UiOrigin,
        commands: Vec<crate::ui::BuiltinInvocation>,
    },
    UpdateKeymap {
        request: u64,
        bindings: Vec<crate::ui::PluginKeybinding>,
    },
}

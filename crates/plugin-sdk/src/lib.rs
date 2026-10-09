//! The JSON protocol and Rust guest SDK for Mitos WebAssembly plugins.
//!
//! Plugins are ordinary core WebAssembly modules, without WASI or host imports.
//! Use [`export_plugin!`] to export the allocation and request entry points.
//! Document offsets count Unicode scalar values; they are not UTF-8 byte offsets.

use serde::{Deserialize, Serialize};

/// Version of the memory ABI and JSON protocol implemented by this SDK.
pub const ABI_VERSION: u32 = 2;

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
    pub text: String,
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
}

#[cfg(any(target_arch = "wasm32", test))]
fn dispatch_request(input: &[u8], handler: fn(Request) -> Response) -> Response {
    match serde_json::from_slice::<Request>(input) {
        Ok(request) if request.abi_version == ABI_VERSION => handler(request),
        Ok(request) => Response {
            error: Some(format!(
                "Unsupported plugin ABI version {}",
                request.abi_version
            )),
            ..Response::default()
        },
        Err(error) => Response {
            error: Some(format!("Invalid plugin request: {error}")),
            ..Response::default()
        },
    }
}

/// Export the Mitos memory ABI for a `fn(Request) -> Response` handler.
///
/// ```
/// use plugin_sdk::{Request, Response, export_plugin};
///
/// fn handle(_request: Request) -> Response {
///     Response::default()
/// }
///
/// export_plugin!(handle);
/// ```
///
/// The exports are emitted only for `wasm32`, allowing the handler to be tested
/// natively. Compile a `cdylib` for `wasm32-unknown-unknown` to produce a plugin.
#[macro_export]
macro_rules! export_plugin {
    ($handler:path) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn mitos_alloc(length: i32) -> i32 {
            $crate::guest::allocate(length)
        }

        /// # Safety
        /// The pointer and length must describe a live allocation returned by
        /// `mitos_alloc` or `mitos_call`, and may be freed only once.
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn mitos_dealloc(pointer: i32, length: i32) {
            // SAFETY: The host owns the buffer and fulfills the ABI contract.
            unsafe { $crate::guest::deallocate(pointer, length) }
        }

        /// # Safety
        /// The pointer and length must describe a live `mitos_alloc` buffer
        /// containing the JSON request. The host retains ownership of it.
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn mitos_call(pointer: i32, length: i32) -> i64 {
            // SAFETY: The host supplies a readable buffer for this invocation.
            unsafe { $crate::guest::call(pointer, length, $handler) }
        }
    };
}

/// Implementation details used by [`export_plugin!`].
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub mod guest {
    use super::{dispatch_request, Request, Response};

    pub fn allocate(length: i32) -> i32 {
        if length <= 0 {
            return 0;
        }
        let buffer = vec![0_u8; length as usize].into_boxed_slice();
        Box::into_raw(buffer).cast::<u8>() as usize as i32
    }

    /// # Safety
    /// `pointer` and `length` must identify a live boxed byte buffer returned by
    /// this module. It must not have been freed, and may be deallocated once.
    pub unsafe fn deallocate(pointer: i32, length: i32) {
        if pointer == 0 || length <= 0 {
            return;
        }
        let pointer = pointer as u32 as *mut u8;
        let buffer = std::ptr::slice_from_raw_parts_mut(pointer, length as usize);
        // SAFETY: The ABI passes the exact allocation address and length. Every
        // buffer exported by this module was allocated as a boxed byte slice.
        drop(unsafe { Box::from_raw(buffer) });
    }

    /// # Safety
    /// `pointer` and `length` must identify a readable allocation returned by
    /// `allocate`. The host keeps that allocation alive until this call returns.
    pub unsafe fn call(pointer: i32, length: i32, handler: fn(Request) -> Response) -> i64 {
        let response = if pointer == 0 || length <= 0 {
            Response {
                error: Some("Invalid plugin request buffer".into()),
                ..Response::default()
            }
        } else {
            // SAFETY: The host follows the documented allocation contract. We
            // borrow only for deserialization and never free the input buffer.
            let input =
                unsafe { std::slice::from_raw_parts(pointer as u32 as *const u8, length as usize) };
            dispatch_request(input, handler)
        };
        // Response contains only JSON-representable values; serialization cannot
        // encounter unsupported map keys, custom serializers, or non-finite floats.
        let output = serde_json::to_vec(&response).expect("plugin response is JSON serializable");
        if output.len() > i32::MAX as usize {
            return 0;
        }
        let length = output.len() as u32;
        let pointer = Box::into_raw(output.into_boxed_slice()).cast::<u8>() as usize as u32;
        ((u64::from(pointer) << 32) | u64::from(length)) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_default_optional_invocation_fields() {
        let request: Request = serde_json::from_value(json!({
            "abi_version": ABI_VERSION,
            "event": "document-changed",
            "editor": {"generation": 1, "mode": "normal", "document": null, "view": null}
        }))
        .unwrap();
        assert_eq!(request.event, Event::DocumentChanged);
        assert_eq!(request.command, None);
        assert!(request.args.is_empty());
        assert_eq!(request.config, serde_json::Value::Null);
        assert_eq!(request.data, serde_json::Value::Null);
        assert_eq!(
            serde_json::from_str::<Response>("{}").unwrap(),
            Response::default()
        );
    }

    #[test]
    fn invalid_json_and_unsupported_abi_do_not_invoke_the_handler() {
        fn unexpected_handler(_request: Request) -> Response {
            panic!("Invalid request must not be dispatched")
        }

        let malformed = dispatch_request(b"not JSON", unexpected_handler);
        assert!(malformed.error.unwrap().contains("Invalid plugin request"));
        assert!(malformed.actions.is_empty());

        let unsupported = dispatch_request(
            br#"{"abi_version":1,"event":"init","editor":{"generation":1,"mode":"normal","document":null,"view":null}}"#,
            unexpected_handler,
        );
        assert_eq!(
            unsupported.error.as_deref(),
            Some("Unsupported plugin ABI version 1")
        );
        assert!(unsupported.actions.is_empty());
    }

    #[test]
    fn response_uses_language_neutral_tagged_actions() {
        let response = Response {
            actions: vec![
                Action::Edit {
                    document: 7,
                    version: 3,
                    edits: vec![TextEdit {
                        start: 1,
                        end: 2,
                        text: "É".into(),
                    }],
                },
                Action::SetSelection {
                    document: 7,
                    version: 3,
                    view: 9,
                    binding_revision: 1,
                    selection_revision: 2,
                    ranges: vec![SelectionRange { anchor: 2, head: 1 }],
                    primary: 0,
                },
            ],
            error: None,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["actions"][0]["type"], "edit");
        assert_eq!(json["actions"][1]["type"], "set-selection");
        assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
    }
}

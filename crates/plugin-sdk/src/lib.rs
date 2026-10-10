//! Rust adapters for the versioned Mitos component world.
//!
//! Use [`export_component!`] or its [`export_plugin!`] alias to export a handler.
//! Compile to `wasm32-unknown-unknown`, then package the core module into a
//! component. The host supplies bounded, capability-checked imports without WASI.
//! Document offsets count Unicode scalar values, not UTF-8 byte offsets.

pub use plugin_api::protocol::*;
pub use plugin_api::{editor, ui, ErrorCode, JobOutput, JobPoll, JobRequest, ServiceError};
pub mod component;
pub mod transform;

/// Export a handler through the typed component interface.
///
/// ```
/// use plugin_sdk::{Request, Response, export_plugin};
/// fn handle(_request: Request) -> Response { Response::default() }
/// export_plugin!(handle);
/// ```
#[macro_export]
macro_rules! export_plugin {
    ($handler:path) => {
        $crate::export_component!($handler);
    };
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

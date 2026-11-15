//! Language-server integration with editor state and operations.

mod capabilities;
pub use capabilities::supports_feature;
pub mod edits;
mod registry;
pub(crate) use registry::Registry;
pub use registry::{has_required_root, language_server_root};

/// Keep editor configuration independent of wire-protocol types.
pub(crate) fn trace_value(trace: editor_config::LspTrace) -> lsp_client::lsp::TraceValue {
    match trace {
        editor_config::LspTrace::Off => lsp_client::lsp::TraceValue::Off,
        editor_config::LspTrace::Messages => lsp_client::lsp::TraceValue::Messages,
        editor_config::LspTrace::Verbose => lsp_client::lsp::TraceValue::Verbose,
    }
}

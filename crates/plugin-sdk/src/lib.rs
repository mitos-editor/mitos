//! Rust adapters for the versioned Mitos component world.
//!
//! Use [`export_component!`] or its [`export_plugin!`] alias to export a handler.
//! Compile to `wasm32-unknown-unknown`, then package the core module into a
//! component. The host supplies bounded, capability-checked imports without WASI.
//! Document offsets count Unicode scalar values, not UTF-8 byte offsets.

pub use plugin_api::protocol::*;
pub use plugin_api::{
    editor, ui, ErrorCode, JobOutput, JobPoll, JobRequest, ReadRoot, ServiceError,
};
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

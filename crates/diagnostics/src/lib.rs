//! Diagnostic values and sorted collections shared across editor features.
//!
//! Positions use document character indices. Collection operations preserve
//! provider namespaces and map findings through text changes. Protocol conversion,
//! asynchronous requests, and editor publication belong to the caller.

mod model;
mod store;

pub use model::{
    Diagnostic, DiagnosticProvider, DiagnosticTag, LanguageServerId, NumberOrString, Range,
    Severity,
};
pub use store::DiagnosticStore;

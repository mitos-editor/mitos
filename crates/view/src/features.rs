//! Feature-owned state, requests, and lifecycle coordination.
//!
//! Documents own feature state; editor handlers own callback destinations.
//! Frontends trigger operations and consume validated updates or cached data.

pub mod hover;
pub mod inlay_hints;
pub mod navigation;
pub(crate) mod request;

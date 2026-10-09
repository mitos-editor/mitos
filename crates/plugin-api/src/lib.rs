//! Runtime-neutral plugin interfaces and host authorization policy.

mod capabilities;
mod services;
pub mod editor;
pub mod protocol;
pub mod ui;

pub use capabilities::{Capability, CapabilitySet, ErrorCode, Permissions, ProcessGrant, ServiceError};
pub use protocol::*;
pub use services::{HostFuture, HostJob, HostServices, JobOutput, JobPoll, JobRequest, ReadRequest, SearchMatch};

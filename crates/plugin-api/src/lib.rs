//! Runtime-neutral plugin interfaces and host authorization policy.

include!(concat!(env!("OUT_DIR"), "/wit_bindings.rs"));

mod capabilities;
pub mod commands;
pub mod editor;
pub mod protocol;
mod services;
pub mod ui;

pub use capabilities::{
    Capability, CapabilitySet, ErrorCode, Permissions, ProcessGrant, ServiceError,
};
pub use protocol::*;
pub use services::{
    HostFuture, HostJob, HostServices, JobOutput, JobPoll, JobRequest, ReadRequest, SearchMatch,
};

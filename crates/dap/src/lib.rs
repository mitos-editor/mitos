//! Asynchronous Debug Adapter Protocol client.
//!
//! [`Client`] owns one adapter process or TCP connection and sends typed DAP
//! requests through [`Transport`]. Incoming adapter events and reverse requests
//! are returned through the receiver created with the client; responses to
//! client-originated requests are matched internally by sequence number.
//!
//! [`registry::Registry`] is the editor-facing owner for multiple clients. It
//! merges their incoming streams and tracks which session is currently active.
//! The protocol data structures themselves are re-exported from `dap-types`.
//!
//! DAP adapters are less uniform than language servers. Keep adapter-specific
//! behavior in [`DebuggerQuirks`] or configuration rather than branching on an
//! adapter name in the transport.

mod client;
pub mod registry;
mod transport;

pub use client::{Client, RequestSender};
pub use dap_types::*;
pub use transport::{Payload, Response, Transport, TransportSender};

use serde::de::DeserializeOwned;
use std::collections::HashMap;

use thiserror::Error;
#[derive(Error, Debug)]
/// Errors raised while starting an adapter, exchanging messages, or decoding
/// protocol payloads.
pub enum Error {
    #[error("failed to parse: {0}")]
    Parse(Box<dyn std::error::Error + Send + Sync>),
    #[error("IO Error: {0}")]
    IO(#[from] std::io::Error),
    #[error("request {0} timed out")]
    Timeout(u64),
    #[error("server closed the stream")]
    StreamClosed,
    #[error("Unhandled")]
    Unhandled,
    #[error(transparent)]
    ExecutableNotFound(#[from] stdx::env::ExecutableNotFoundError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
/// Result type used by DAP client and transport operations.
pub type Result<T> = core::result::Result<T, Error>;

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Parse(Box::new(value))
    }
}

impl From<sonic_rs::Error> for Error {
    fn from(value: sonic_rs::Error) -> Self {
        Self::Parse(Box::new(value))
    }
}

#[derive(Debug)]
/// Reverse requests that Mitos knows how to handle from a debug adapter.
pub enum Request {
    RunInTerminal(<requests::RunInTerminal as dap_types::Request>::Arguments),
    StartDebugging(<requests::StartDebugging as dap_types::Request>::Arguments),
}

impl Request {
    /// Decodes a reverse request by its wire command name.
    ///
    /// Missing arguments are decoded from JSON `null`. Unknown commands return
    /// [`Error::Unhandled`] so the caller can produce a protocol response.
    pub fn parse(command: &str, arguments: Option<serde_json::Value>) -> Result<Self> {
        use dap_types::Request as _;

        let arguments = arguments.unwrap_or_default();
        let request = match command {
            requests::RunInTerminal::COMMAND => Self::RunInTerminal(parse_value(arguments)?),
            requests::StartDebugging::COMMAND => Self::StartDebugging(parse_value(arguments)?),
            _ => return Err(Error::Unhandled),
        };

        Ok(request)
    }
}

#[derive(Debug)]
/// Debug adapter events handled by the editor.
pub enum Event {
    Initialized(<events::Initialized as events::Event>::Body),
    Stopped(<events::Stopped as events::Event>::Body),
    Continued(<events::Continued as events::Event>::Body),
    Exited(<events::Exited as events::Event>::Body),
    Terminated(<events::Terminated as events::Event>::Body),
    Thread(<events::Thread as events::Event>::Body),
    Output(<events::Output as events::Event>::Body),
    Breakpoint(<events::Breakpoint as events::Event>::Body),
    Module(<events::Module as events::Event>::Body),
    LoadedSource(<events::LoadedSource as events::Event>::Body),
    Process(<events::Process as events::Event>::Body),
    Capabilities(<events::Capabilities as events::Event>::Body),
    ProgressStart(<events::ProgressStart as events::Event>::Body),
    ProgressUpdate(<events::ProgressUpdate as events::Event>::Body),
    ProgressEnd(<events::ProgressEnd as events::Event>::Body),
    // Invalidated(),
    Memory(<events::Memory as events::Event>::Body),
}

impl Event {
    /// Decodes an event by its wire event name.
    ///
    /// Missing bodies are decoded from JSON `null`, and unknown events return
    /// [`Error::Unhandled`].
    pub fn parse(event: &str, body: Option<serde_json::Value>) -> Result<Self> {
        use crate::events::Event as _;

        let body = body.unwrap_or_default();
        let event = match event {
            events::Initialized::EVENT => Self::Initialized(parse_value(body)?),
            events::Stopped::EVENT => Self::Stopped(parse_value(body)?),
            events::Continued::EVENT => Self::Continued(parse_value(body)?),
            events::Exited::EVENT => Self::Exited(parse_value(body)?),
            events::Terminated::EVENT => Self::Terminated(parse_value(body)?),
            events::Thread::EVENT => Self::Thread(parse_value(body)?),
            events::Output::EVENT => Self::Output(parse_value(body)?),
            events::Breakpoint::EVENT => Self::Breakpoint(parse_value(body)?),
            events::Module::EVENT => Self::Module(parse_value(body)?),
            events::LoadedSource::EVENT => Self::LoadedSource(parse_value(body)?),
            events::Process::EVENT => Self::Process(parse_value(body)?),
            events::Capabilities::EVENT => Self::Capabilities(parse_value(body)?),
            events::ProgressStart::EVENT => Self::ProgressStart(parse_value(body)?),
            events::ProgressUpdate::EVENT => Self::ProgressUpdate(parse_value(body)?),
            events::ProgressEnd::EVENT => Self::ProgressEnd(parse_value(body)?),
            events::Memory::EVENT => Self::Memory(parse_value(body)?),
            _ => return Err(Error::Unhandled),
        };

        Ok(event)
    }
}

fn parse_value<T>(value: serde_json::Value) -> Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(value).map_err(|err| err.into())
}

#[derive(Debug, Clone)]
/// User-visible state for one long-running DAP progress report.
pub struct ProgressState {
    title: String,
    message: Option<String>,
    percentage: Option<u8>,
}

impl ProgressState {
    /// Starts a progress report with the adapter-provided display fields.
    pub fn new(title: String, message: Option<String>, percentage: Option<u8>) -> Self {
        Self {
            title,
            message,
            percentage,
        }
    }

    /// Applies an incremental progress update.
    ///
    /// DAP omits unchanged fields, so `None` preserves the previous value
    /// rather than clearing it.
    pub fn update(&mut self, message: Option<String>, percentage: Option<u8>) {
        if let Some(message) = message {
            self.message = Some(message);
        }
        if let Some(percentage) = percentage {
            self.percentage = Some(percentage);
        }
    }

    /// Formats the in-progress status shown by the editor.
    pub fn status_line(&self) -> String {
        let mut status = format!("Debug: {}", self.title);
        if let Some(message) = self.message.as_deref() {
            status.push_str(" - ");
            status.push_str(message);
        }
        if let Some(percentage) = self.percentage {
            status.push_str(&format!(" ({}%)", percentage));
        }
        status
    }

    /// Formats the final status, preferring the event's final message over the
    /// most recent incremental message.
    pub fn end_status_line(&self, message: Option<&str>) -> String {
        let mut status = format!("Debug: {}", self.title);
        if let Some(message) = message.or(self.message.as_deref()) {
            status.push_str(" - ");
            status.push_str(message);
        } else {
            status.push_str(" finished");
        }
        status
    }
}

/// Active DAP progress reports keyed by the adapter's progress identifier.
pub type ProgressMap = HashMap<String, ProgressState>;

//! Asynchronous Debug Adapter Protocol client.
//!
//! [`Client`] owns one adapter process or TCP connection and sends typed DAP
//! requests through [`Transport`]. Incoming adapter events and reverse requests
//! are returned through the receiver created with the client; responses to
//! client-originated requests are matched internally by sequence number.
//!
//! Session membership, execution state, and active selection belong to the editor.
//! Protocol data structures are re-exported from `dap-types`.
//!
//! Capabilities, launch arguments, and restart configuration belong to the session
//! owner, alongside adapter selection.
//! The client accepts explicit stream, stdio, or TCP connections.

mod client;
mod error;
pub mod registry;
mod transport;

pub use client::{Client, RequestSender};
pub use dap_types::*;
pub use error::RequestError;
pub use registry::DebugAdapterId;
pub use transport::{ClientEvent, Payload, Response, Transport, TransportSender};

use serde::de::DeserializeOwned;
use thiserror::Error;

#[derive(Error, Debug)]
/// Errors raised while starting an adapter, exchanging messages, or decoding
/// protocol payloads.
pub enum Error {
    #[error(transparent)]
    Request(#[from] RequestError),
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

impl From<protocol_io::ReadError> for Error {
    fn from(error: protocol_io::ReadError) -> Self {
        match error {
            protocol_io::ReadError::Closed => Self::StreamClosed,
            protocol_io::ReadError::Io(error) => Self::IO(error),
            error => Self::Other(error.into()),
        }
    }
}

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

macro_rules! decoded_events {
    ($($event:ident),* $(,)?) => {
        /// Debug adapter events handled by the editor.
        #[derive(Debug)]
        pub enum Event { $($event(<events::$event as events::Event>::Body),)* }
        impl Event {
            /// Decode the event body; unknown event names remain unhandled.
            pub fn parse(event: &str, body: Option<serde_json::Value>) -> Result<Self> {
                use events::Event as _;
                let body = body.unwrap_or_default();
                Ok(match event {
                    $(events::$event::EVENT => Self::$event(parse_value(body)?),)*
                    _ => return Err(Error::Unhandled),
                })
            }
        }
    }
}
decoded_events! {
    Initialized,
    Stopped,
    Continued,
    Exited,
    Terminated,
    Thread,
    Output,
    Breakpoint,
    Module,
    LoadedSource,
    Process,
    Capabilities,
    ProgressStart,
    ProgressUpdate,
    ProgressEnd,
    Invalidated,
    Memory,
}

fn parse_value<T>(value: serde_json::Value) -> Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(value).map_err(|err| err.into())
}

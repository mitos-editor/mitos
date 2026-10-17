//! Clipboard provider settings, backend interface, and native platform access.

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardType {
    Clipboard,
    Selection,
}

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error("could not convert terminal output to UTF-8: {0}")]
    FromUtf8Error(#[from] std::string::FromUtf8Error),
    #[cfg(windows)]
    #[error("Windows API error: {0}")]
    WinAPI(#[from] clipboard_win::ErrorCode),
    #[error("clipboard provider command failed")]
    CommandFailed,
    #[error("failed to write to clipboard provider's stdin")]
    StdinWriteFailed,
    #[error("clipboard provider did not return any contents")]
    MissingStdout,
    #[error("This clipboard provider does not support reading")]
    ReadingNotSupported,
    #[error("clipboard provider requires frontend support")]
    Unavailable,
}

pub type Result<T> = std::result::Result<T, ClipboardError>;

mod config;
#[cfg(not(target_arch = "wasm32"))]
mod native;

pub use config::{ClipboardProvider, Command, CommandProvider};
#[cfg(not(target_arch = "wasm32"))]
pub use native::NativeClipboard;

/// Clipboard access supplied by the application. Settings are a snapshot for each operation.
/// Implementations must not retain a reference to the supplied settings.
pub trait ClipboardBackend: Send + Sync {
    fn name(&self, provider: &ClipboardProvider) -> String;
    fn get_contents(&self, provider: &ClipboardProvider, kind: ClipboardType) -> Result<String>;
    fn set_contents(
        &self,
        provider: &ClipboardProvider,
        content: &str,
        kind: ClipboardType,
    ) -> Result<()>;
}

#[cfg(target_arch = "wasm32")]
pub struct NativeClipboard;

#[cfg(target_arch = "wasm32")]
impl ClipboardBackend for NativeClipboard {
    fn name(&self, _: &ClipboardProvider) -> String {
        "none".into()
    }
    fn get_contents(&self, _: &ClipboardProvider, _: ClipboardType) -> Result<String> {
        Err(ClipboardError::ReadingNotSupported)
    }
    fn set_contents(&self, _: &ClipboardProvider, _: &str, _: ClipboardType) -> Result<()> {
        Ok(())
    }
}

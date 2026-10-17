//! Editor-owned clipboard access with live configuration.

use ::clipboard::{ClipboardBackend, ClipboardProvider, ClipboardType, NativeClipboard, Result};
use arc_swap::access::DynAccess;

/// Editor-owned clipboard access. Configuration changes are visible on the next operation.
pub struct Clipboard {
    config: Box<dyn DynAccess<ClipboardProvider>>,
    backend: Box<dyn ClipboardBackend>,
}

impl Clipboard {
    pub fn new(
        config: Box<dyn DynAccess<ClipboardProvider>>,
        backend: Box<dyn ClipboardBackend>,
    ) -> Self {
        Self { config, backend }
    }

    pub fn native(config: Box<dyn DynAccess<ClipboardProvider>>) -> Self {
        Self::new(config, Box::new(NativeClipboard))
    }

    pub fn set_backend(&mut self, backend: Box<dyn ClipboardBackend>) {
        self.backend = backend;
    }

    pub fn name(&self) -> String {
        self.backend.name(&self.config.load())
    }

    pub fn get_contents(&self, kind: ClipboardType) -> Result<String> {
        self.backend.get_contents(&self.config.load(), kind)
    }

    pub fn set_contents(&self, content: &str, kind: ClipboardType) -> Result<()> {
        self.backend
            .set_contents(&self.config.load(), content, kind)
    }
}

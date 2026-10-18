//! Document-facing diagnostic operations backed by the shared collection.

use crate::Document;
use ::diagnostics::{Diagnostic, DiagnosticProvider, LanguageServerId};

impl Document {
    pub fn diagnostics(&self) -> &[Diagnostic] {
        self.diagnostics.as_slice()
    }

    pub fn replace_diagnostics(
        &mut self,
        diagnostics: impl IntoIterator<Item = Diagnostic>,
        unchanged_sources: &[String],
        provider: Option<&DiagnosticProvider>,
    ) {
        self.diagnostics
            .replace(diagnostics, unchanged_sources, provider);
    }

    /// Refresh cached LSP diagnostics, preserving other providers.
    pub fn replace_language_server_diagnostics(
        &mut self,
        diagnostics: impl IntoIterator<Item = Diagnostic>,
    ) {
        self.diagnostics.replace_language_server(diagnostics);
    }

    /// Replace this provider's findings overlapping the supplied character ranges.
    pub fn splice_diagnostics(
        &mut self,
        diagnostics: impl IntoIterator<Item = Diagnostic>,
        regions: &[std::ops::Range<usize>],
        provider: &DiagnosticProvider,
    ) {
        self.diagnostics.splice(diagnostics, regions, provider);
    }

    pub fn clear_diagnostics_for_language_server(&mut self, id: LanguageServerId) {
        self.diagnostics.clear_language_server(id);
    }
}

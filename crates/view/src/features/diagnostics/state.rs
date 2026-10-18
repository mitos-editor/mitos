//! Document diagnostic storage, provider replacement, and edit mapping.

use crate::Document;
use editor_core::{diagnostic::DiagnosticProvider, Assoc, ChangeSet, Diagnostic, Rope};
use lsp_client::LanguageServerId;

#[derive(Debug, Default)]
pub(crate) struct DiagnosticStore {
    entries: Vec<Diagnostic>,
}

impl DiagnosticStore {
    fn sort(&mut self) {
        self.entries.sort_by(|a, b| {
            (a.range, a.severity, &a.provider).cmp(&(b.range, b.severity, &b.provider))
        });
    }

    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet, text: &Rope) {
        changes.update_positions(self.entries.iter_mut().map(|diagnostic| {
            let assoc = if diagnostic.starts_at_word {
                Assoc::BeforeWord
            } else {
                Assoc::After
            };
            (&mut diagnostic.range.start, assoc)
        }));
        changes.update_positions(self.entries.iter_mut().filter_map(|diagnostic| {
            if diagnostic.zero_width {
                // for zero width diagnostics treat the diagnostic as a point
                // rather than a range
                return None;
            }
            let assoc = if diagnostic.ends_at_word {
                Assoc::AfterWord
            } else {
                Assoc::Before
            };
            Some((&mut diagnostic.range.end, assoc))
        }));
        self.entries.retain_mut(|diagnostic| {
            if diagnostic.zero_width {
                diagnostic.range.end = diagnostic.range.start
            } else if diagnostic.range.start >= diagnostic.range.end {
                return false;
            }
            diagnostic.line = text.char_to_line(diagnostic.range.start);
            true
        });

        self.sort();
    }
}

impl Document {
    #[inline]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics.entries
    }

    pub fn replace_diagnostics(
        &mut self,
        diagnostics: impl IntoIterator<Item = Diagnostic>,
        unchanged_sources: &[String],
        provider: Option<&DiagnosticProvider>,
    ) {
        if unchanged_sources.is_empty() {
            if let Some(provider) = provider {
                self.diagnostics
                    .entries
                    .retain(|diagnostic| &diagnostic.provider != provider);
            } else {
                self.diagnostics.entries.clear();
            }
        } else {
            self.diagnostics.entries.retain(|d| {
                if provider.is_some_and(|provider| provider != &d.provider) {
                    return true;
                }

                d.source
                    .as_ref()
                    .is_some_and(|source| unchanged_sources.iter().any(|s| **s == **source))
            });
        }
        self.extend_diagnostics(diagnostics);
    }

    /// Refresh cached language-server diagnostics without discarding diagnostics
    /// owned by other providers.
    pub fn replace_language_server_diagnostics(
        &mut self,
        diagnostics: impl IntoIterator<Item = Diagnostic>,
    ) {
        self.diagnostics
            .entries
            .retain(|diagnostic| !matches!(diagnostic.provider, DiagnosticProvider::Lsp { .. }));
        self.extend_diagnostics(diagnostics);
    }

    fn extend_diagnostics(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.diagnostics.entries.extend(diagnostics);
        self.diagnostics.sort();
    }

    /// Replaces `provider`'s diagnostics that fall within any of `regions` with `diagnostics`,
    /// leaving every other diagnostic untouched, including `provider`'s own diagnostics that lie
    /// outside `regions`.
    ///
    /// This is the incremental counterpart to [`Document::replace_diagnostics`]: when only bounded
    /// regions of the document have been re-examined (e.g. the areas around an edit), the caller
    /// re-checks those regions and splices the results in, rather than recomputing the whole
    /// document. Diagnostics outside `regions` keep the positions they were edit-mapped to.
    ///
    /// `regions` are half-open char ranges in the current document text; `diagnostics` are expected
    /// to fall within them. A diagnostic is considered inside a region when their ranges overlap.
    pub fn splice_diagnostics(
        &mut self,
        diagnostics: impl IntoIterator<Item = Diagnostic>,
        regions: &[std::ops::Range<usize>],
        provider: &DiagnosticProvider,
    ) {
        self.diagnostics.entries.retain(|d| {
            &d.provider != provider
                || !regions
                    .iter()
                    .any(|region| d.range.start < region.end && region.start < d.range.end)
        });
        self.extend_diagnostics(diagnostics);
    }

    /// Clear every diagnostic namespace owned by this language server.
    pub fn clear_diagnostics_for_language_server(&mut self, id: LanguageServerId) {
        self.diagnostics
            .entries
            .retain(|d| d.provider.language_server_id() != Some(id));
    }
}

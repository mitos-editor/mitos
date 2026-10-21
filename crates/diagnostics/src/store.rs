//! Sorted diagnostic collections with provider-scoped replacement and edit mapping.

use crate::{Diagnostic, DiagnosticProvider, LanguageServerId};
use editor_core::{Assoc, ChangeSet, Rope};
use stdx::rope::RopeExt as _;

#[derive(Debug, Default, Clone)]
pub struct DiagnosticStore {
    entries: Vec<Diagnostic>,
}

impl DiagnosticStore {
    fn sort(&mut self) {
        self.entries.sort_by(|a, b| {
            (a.range, a.severity, &a.provider).cmp(&(b.range, b.severity, &b.provider))
        });
    }

    /// Map findings to the supplied post-edit text, dropping collapsed ranges.
    pub fn apply_changes(&mut self, changes: &ChangeSet, text: &Rope) {
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
            diagnostic.line = text.char_to_line_idx(diagnostic.range.start);
            true
        });

        self.sort();
    }

    #[inline]
    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.entries
    }

    pub fn replace(
        &mut self,
        diagnostics: impl IntoIterator<Item = Diagnostic>,
        unchanged_sources: &[String],
        provider: Option<&DiagnosticProvider>,
    ) {
        if unchanged_sources.is_empty() {
            if let Some(provider) = provider {
                self.entries
                    .retain(|diagnostic| &diagnostic.provider != provider);
            } else {
                self.entries.clear();
            }
        } else {
            self.entries.retain(|d| {
                if provider.is_some_and(|provider| provider != &d.provider) {
                    return true;
                }

                d.source
                    .as_ref()
                    .is_some_and(|source| unchanged_sources.iter().any(|s| **s == **source))
            });
        }
        self.extend(diagnostics);
    }

    /// Refresh cached language-server diagnostics without discarding diagnostics
    /// owned by other providers.
    pub fn replace_language_server(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.entries
            .retain(|diagnostic| !matches!(diagnostic.provider, DiagnosticProvider::Lsp { .. }));
        self.extend(diagnostics);
    }

    fn extend(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.entries.extend(diagnostics);
        self.sort();
    }

    /// Replaces `provider`'s diagnostics that fall within any of `regions` with `diagnostics`,
    /// leaving every other diagnostic untouched, including `provider`'s own diagnostics that lie
    /// outside `regions`.
    ///
    /// This is the incremental counterpart to [`Self::replace`]: when only bounded
    /// regions of the document have been re-examined (e.g. the areas around an edit), the caller
    /// re-checks those regions and splices the results in, rather than recomputing the whole
    /// document. Diagnostics outside `regions` keep the positions they were edit-mapped to.
    ///
    /// `regions` are half-open char ranges in the current document text; `diagnostics` are expected
    /// to fall within them. A diagnostic is considered inside a region when their ranges overlap.
    pub fn splice(
        &mut self,
        diagnostics: impl IntoIterator<Item = Diagnostic>,
        regions: &[std::ops::Range<usize>],
        provider: &DiagnosticProvider,
    ) {
        self.entries.retain(|d| {
            &d.provider != provider
                || !regions
                    .iter()
                    .any(|region| d.range.start < region.end && region.start < d.range.end)
        });
        self.extend(diagnostics);
    }

    /// Clear every diagnostic namespace owned by this language server.
    pub fn clear_language_server(&mut self, id: LanguageServerId) {
        self.entries
            .retain(|d| d.provider.language_server_id() != Some(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Range, Severity};

    fn finding(start: usize, end: usize, provider: DiagnosticProvider) -> Diagnostic {
        Diagnostic {
            range: Range { start, end },
            starts_at_word: true,
            ends_at_word: true,
            zero_width: false,
            line: 0,
            message: "finding".into(),
            severity: Some(Severity::Warning),
            code: None,
            provider,
            tags: Vec::new(),
            source: None,
            data: None,
        }
    }

    #[test]
    fn bounded_splicing_preserves_other_providers_and_outside_findings() {
        let lsp = DiagnosticProvider::Lsp {
            server_id: LanguageServerId::default(),
            identifier: None,
        };
        let mut store = DiagnosticStore::default();
        store.replace(
            [
                finding(8, 10, DiagnosticProvider::Spelling),
                finding(0, 3, DiagnosticProvider::Spelling),
                finding(1, 2, lsp.clone()),
            ],
            &[],
            None,
        );
        let checked_region = 0..5;
        store.splice(
            [finding(2, 4, DiagnosticProvider::Spelling)],
            std::slice::from_ref(&checked_region),
            &DiagnosticProvider::Spelling,
        );
        let entries = store.as_slice();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].provider, lsp);
        assert_eq!(
            entries.iter().map(|d| d.range).collect::<Vec<_>>(),
            [
                Range { start: 1, end: 2 },
                Range { start: 2, end: 4 },
                Range { start: 8, end: 10 }
            ]
        );
    }
}

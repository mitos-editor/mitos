//! Versioned document annotations with host-assigned plugin ownership.
use std::collections::BTreeSet;

use editor_core::{
    chars::char_is_word,
    diagnostic::{Diagnostic, DiagnosticProvider, Range, Severity},
};
use plugin_api::{
    editor::{DiagnosticSeverity, DocumentDiagnostic, DocumentTarget},
    ErrorCode, PluginOwner, ServiceError,
};

use crate::{Document, DocumentId, Editor};

fn provider(owner: &PluginOwner) -> DiagnosticProvider {
    DiagnosticProvider::Plugin {
        plugin: owner.plugin.clone().into(),
        generation: owner.generation,
    }
}

impl Editor {
    pub(super) fn publish_plugin_document_diagnostics(
        &mut self,
        owner: &PluginOwner,
        target: DocumentTarget,
        diagnostics: Vec<DocumentDiagnostic>,
    ) -> Result<(), ServiceError> {
        let id = self
            .plugin_document(target.document, target.version)
            .map_err(|error| ServiceError::new(ErrorCode::StaleState, error.to_string()))?;
        let doc = self.document_mut(id).unwrap();
        if doc.is_binary() {
            return Err(ServiceError::new(
                ErrorCode::InvalidRequest,
                "diagnostics require a text document",
            ));
        }
        let provider = provider(owner);
        let text = doc.text().slice(..);
        // Prepare the whole replacement before touching the existing annotations.
        let diagnostics = diagnostics
            .into_iter()
            .map(|diagnostic| {
                let start = usize::try_from(diagnostic.range.start).ok();
                let end = usize::try_from(diagnostic.range.end).ok();
                let (Some(start), Some(end)) = (start, end) else {
                    return Err(ServiceError::new(
                        ErrorCode::InvalidRequest,
                        "diagnostic range is out of bounds",
                    ));
                };
                if start > end || end > text.len_chars() {
                    return Err(ServiceError::new(
                        ErrorCode::InvalidRequest,
                        "diagnostic range is out of bounds",
                    ));
                }
                let message = plugin_api::ui::terminal_text(&diagnostic.message, true);
                if message.is_empty() {
                    return Err(ServiceError::new(
                        ErrorCode::InvalidRequest,
                        "diagnostic needs a printable message",
                    ));
                }
                Ok(Diagnostic {
                    range: Range { start, end },
                    starts_at_word: start != end && text.get_char(start).is_some_and(char_is_word),
                    ends_at_word: start != end && text.get_char(end - 1).is_some_and(char_is_word),
                    zero_width: start == end,
                    line: text.char_to_line(start),
                    message: message.into(),
                    severity: Some(match diagnostic.severity {
                        DiagnosticSeverity::Error => Severity::Error,
                        DiagnosticSeverity::Warning => Severity::Warning,
                        DiagnosticSeverity::Info => Severity::Info,
                        DiagnosticSeverity::Hint => Severity::Hint,
                    }),
                    provider: provider.clone(),
                    source: Some(owner.plugin.clone().into()),
                    code: None,
                    tags: Vec::new(),
                    data: None,
                })
            })
            .collect::<Result<Vec<_>, ServiceError>>()?;
        doc.replace_diagnostics(diagnostics, &[], Some(&provider));
        event::dispatch(crate::events::DiagnosticsDidChange {
            editor: self,
            doc: id,
        });
        event::request_redraw();
        Ok(())
    }

    pub(super) fn clear_plugin_document_diagnostics(
        &mut self,
        owner: &PluginOwner,
        target: Option<DocumentTarget>,
    ) -> Result<(), ServiceError> {
        let id = target
            .map(|target| self.plugin_document(target.document, target.version))
            .transpose()
            .map_err(|error| ServiceError::new(ErrorCode::StaleState, error.to_string()))?;
        let provider = provider(owner);
        self.remove_plugin_document_diagnostics(id, |candidate| candidate == &provider);
        Ok(())
    }

    fn remove_plugin_document_diagnostics(
        &mut self,
        target: Option<DocumentId>,
        remove: impl Fn(&DiagnosticProvider) -> bool,
    ) {
        let mut changed = Vec::new();
        let mut retain = |doc: &mut Document| {
            let previous = doc.diagnostics.len();
            doc.diagnostics
                .retain(|diagnostic| !remove(&diagnostic.provider));
            if doc.diagnostics.len() != previous {
                changed.push(doc.id());
            }
        };
        if let Some(target) = target {
            if let Some(doc) = self.documents.get_mut(&target) {
                retain(doc);
            }
        } else {
            for doc in self.documents.values_mut() {
                retain(doc);
            }
        }
        for doc in changed {
            event::dispatch(crate::events::DiagnosticsDidChange { editor: self, doc });
            event::request_redraw();
        }
    }

    pub(super) fn clear_all_plugin_document_diagnostics(&mut self) {
        self.remove_plugin_document_diagnostics(None, |provider| {
            matches!(provider, DiagnosticProvider::Plugin { .. })
        });
    }

    pub(super) fn prune_plugin_document_diagnostics(&mut self) {
        // Deduplicate borrowed providers before testing liveness. Only stale
        // providers need owned keys, and cloning their names shares the Arc.
        let owners = self
            .documents()
            .flat_map(|doc| doc.diagnostics())
            .map(|diagnostic| &diagnostic.provider)
            .filter(|provider| matches!(provider, DiagnosticProvider::Plugin { .. }))
            .collect::<BTreeSet<_>>();
        let stale = owners
            .into_iter()
            .filter(|provider| match provider {
                DiagnosticProvider::Plugin { plugin, generation } => {
                    !self.plugin_identity_is_current(plugin, *generation)
                }
                _ => false,
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        if !stale.is_empty() {
            self.remove_plugin_document_diagnostics(None, |provider| stale.contains(provider));
        }
    }
}

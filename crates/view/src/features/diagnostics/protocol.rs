//! LSP diagnostics conversion, caching, and publication for push and pull providers.

use std::collections::BTreeMap;

use crate::{events::DiagnosticsDidChange, Document, DocumentId, Editor};
use editor_core::{chars::char_is_word, diagnostic::DiagnosticProvider, Diagnostic, Rope, Uri};
use language_config::{LanguageConfiguration, LanguageServerFeature};
use lsp_client::{lsp, util::lsp_pos_to_pos, LanguageServerId};

/// Raw diagnostics retained for open and unopened files, grouped by URI.
pub type LspDiagnostics = BTreeMap<Uri, Vec<(lsp::Diagnostic, DiagnosticProvider)>>;

fn convert_diagnostic(
    text: &Rope,
    language_config: Option<&LanguageConfiguration>,
    diagnostic: &lsp_client::lsp::Diagnostic,
    provider: DiagnosticProvider,
    offset_encoding: lsp_client::OffsetEncoding,
) -> Option<Diagnostic> {
    use editor_core::diagnostic::{Range, Severity::*};

    let start = if let Some(start) = lsp_pos_to_pos(text, diagnostic.range.start, offset_encoding) {
        start
    } else {
        log::warn!("lsp position out of bounds - {:?}", diagnostic);
        return None;
    };

    let end = if let Some(end) = lsp_pos_to_pos(text, diagnostic.range.end, offset_encoding) {
        end
    } else {
        log::warn!("lsp position out of bounds - {:?}", diagnostic);
        return None;
    };

    let severity = diagnostic.severity.and_then(|severity| match severity {
        lsp::DiagnosticSeverity::ERROR => Some(Error),
        lsp::DiagnosticSeverity::WARNING => Some(Warning),
        lsp::DiagnosticSeverity::INFORMATION => Some(Info),
        lsp::DiagnosticSeverity::HINT => Some(Hint),
        severity => {
            log::error!("unrecognized diagnostic severity: {:?}", severity);
            None
        }
    });

    if let Some(lang_conf) = language_config
        && let Some(severity) = severity
        && severity < lang_conf.diagnostic_severity
    {
        return None;
    };
    use editor_core::diagnostic::{DiagnosticTag, NumberOrString};

    let code = diagnostic.code.clone().map(|code| match code {
        lsp::NumberOrString::Number(value) => NumberOrString::Number(value),
        lsp::NumberOrString::String(value) => NumberOrString::String(value.into()),
    });

    let tags = if let Some(tags) = &diagnostic.tags {
        tags.iter()
            .filter_map(|tag| match *tag {
                lsp::DiagnosticTag::DEPRECATED => Some(DiagnosticTag::Deprecated),
                lsp::DiagnosticTag::UNNECESSARY => Some(DiagnosticTag::Unnecessary),
                _ => None,
            })
            .collect()
    } else {
        Vec::new()
    };

    let ends_at_word = start != end && end != 0 && text.get_char(end - 1).is_some_and(char_is_word);
    let starts_at_word = start != end && text.get_char(start).is_some_and(char_is_word);

    Some(Diagnostic {
        range: Range { start, end },
        ends_at_word,
        starts_at_word,
        zero_width: start == end,
        line: diagnostic.range.start.line as usize,
        message: diagnostic.message.clone().into(),
        severity,
        code,
        tags,
        source: diagnostic.source.clone().map(|source| source.into()),
        data: diagnostic.data.clone(),
        provider,
    })
}

impl Editor {
    /// Raw language-server diagnostics for open and unopened files.
    pub fn diagnostics(&self) -> &LspDiagnostics {
        &self.diagnostics
    }

    /// Refresh a document from cached LSP results, preserving other providers.
    pub fn refresh_language_server_diagnostics(&mut self, doc_id: DocumentId) {
        let Some(doc) = self.documents.get_mut(&doc_id) else {
            return;
        };
        let diagnostics = Self::doc_diagnostics(&self.language_servers, &self.diagnostics, doc);
        doc.replace_language_server_diagnostics(diagnostics);
        event::dispatch(DiagnosticsDidChange {
            editor: self,
            doc: doc_id,
        })
        .report_to(self.status_sender());
    }

    /// Validate a push notification before applying the shared diagnostic update policy.
    pub fn handle_publish_diagnostics(
        &mut self,
        server_id: LanguageServerId,
        params: lsp::PublishDiagnosticsParams,
    ) {
        let uri = match Uri::try_from(params.uri) {
            Ok(uri) => uri,
            Err(err) => {
                log::error!("{err}");
                return;
            }
        };
        let Some(language_server) = self.language_server_by_id(server_id) else {
            log::warn!("can't find language server with id `{server_id}`");
            return;
        };
        if !self.language_server_is_ready(server_id) {
            log::error!(
                "Discarding publishDiagnostic notification sent by an uninitialized server: {}",
                language_server.name()
            );
            return;
        }
        let provider = DiagnosticProvider::Lsp {
            server_id,
            identifier: None,
        };
        self.handle_lsp_diagnostics(&provider, uri, params.version, params.diagnostics);
    }

    pub fn handle_lsp_diagnostics(
        &mut self,
        provider: &DiagnosticProvider,
        uri: Uri,
        version: Option<i32>,
        mut diagnostics: Vec<lsp::Diagnostic>,
    ) {
        let doc = self
            .documents
            .values_mut()
            .find(|doc| doc.uri().is_some_and(|u| u == uri));

        if let Some((version, doc)) = version.zip(doc.as_ref())
            && version != doc.version()
        {
            log::info!("Version ({version}) is out of date for {uri:?} (expected ({})), dropping PublishDiagnostic notification", doc.version());
            return;
        }

        let mut unchanged_diag_sources = Vec::new();
        if let Some((lang_conf, old_diagnostics)) = doc
            .as_ref()
            .and_then(|doc| Some((doc.language_config()?, self.diagnostics.get(&uri)?)))
        {
            if !lang_conf.persistent_diagnostic_sources.is_empty() {
                // Sort diagnostics first by severity and then by line numbers.
                // Note: The `lsp::DiagnosticSeverity` enum is already defined in decreasing order
                diagnostics.sort_by_key(|d| (d.severity, d.range.start));
            }
            for source in &lang_conf.persistent_diagnostic_sources {
                let new_diagnostics = diagnostics
                    .iter()
                    .filter(|d| d.source.as_ref() == Some(source));
                let old_diagnostics = old_diagnostics
                    .iter()
                    .filter(|(d, d_provider)| {
                        d_provider == provider && d.source.as_ref() == Some(source)
                    })
                    .map(|(d, _)| d);
                if new_diagnostics.eq(old_diagnostics) {
                    unchanged_diag_sources.push(source.clone())
                }
            }
        }

        let diagnostics = diagnostics.into_iter().map(|d| (d, provider.clone()));

        // Keep raw positions for unopened files; convert when a document is available.
        let current = self.diagnostics.entry(uri).or_default();
        current.retain(|(_, current_provider)| current_provider != provider);
        current.extend(diagnostics);
        // LSP severities sort from most to least severe, then by position and provider.
        current.sort_by(|(a, a_provider), (b, b_provider)| {
            (a.severity, a.range.start, a_provider).cmp(&(b.severity, b.range.start, b_provider))
        });

        if let Some(doc) = doc {
            let diagnostic_of_language_server_and_not_in_unchanged_sources =
                |diagnostic: &lsp::Diagnostic, d_provider: &DiagnosticProvider| {
                    d_provider == provider
                        && diagnostic
                            .source
                            .as_ref()
                            .is_none_or(|source| !unchanged_diag_sources.contains(source))
                };
            let diagnostics = Self::doc_diagnostics_with_filter(
                &self.language_servers,
                &self.diagnostics,
                doc,
                diagnostic_of_language_server_and_not_in_unchanged_sources,
            );
            doc.replace_diagnostics(diagnostics, &unchanged_diag_sources, Some(provider));

            let doc = doc.id();
            event::dispatch(DiagnosticsDidChange { editor: self, doc })
                .report_to(self.status_sender());
        }
    }

    /// Returns all supported diagnostics for the document
    pub fn doc_diagnostics<'a>(
        language_servers: &'a crate::language_servers::Registry,
        diagnostics: &'a LspDiagnostics,
        document: &Document,
    ) -> impl Iterator<Item = editor_core::Diagnostic> + 'a + use<'a> {
        Editor::doc_diagnostics_with_filter(language_servers, diagnostics, document, |_, _| true)
    }

    /// Returns all supported diagnostics for the document
    /// filtered by the raw LSP diagnostic and its provider
    pub fn doc_diagnostics_with_filter<'a, F>(
        language_servers: &'a crate::language_servers::Registry,
        diagnostics: &'a LspDiagnostics,
        document: &Document,
        filter: F,
    ) -> impl Iterator<Item = editor_core::Diagnostic> + 'a + use<'a, F>
    where
        F: Fn(&lsp::Diagnostic, &DiagnosticProvider) -> bool + 'a,
    {
        let text = document.text().clone();
        let language_config = document.language.clone();
        document
            .uri()
            .and_then(|uri| diagnostics.get(&uri))
            .map(|diags| {
                diags.iter().filter_map(move |(diagnostic, provider)| {
                    let server_id = provider.language_server_id()?;
                    let ls = language_servers.get_by_id(server_id)?;
                    let config = language_config.as_deref()?;
                    let enabled = config.language_servers.iter().any(|features| {
                        features.name == ls.name()
                            && features.has_feature(LanguageServerFeature::Diagnostics)
                    });
                    if !enabled || !filter(diagnostic, provider) {
                        return None;
                    }
                    convert_diagnostic(
                        &text,
                        Some(config),
                        diagnostic,
                        provider.clone(),
                        ls.offset_encoding(),
                    )
                })
            })
            .into_iter()
            .flatten()
    }

    pub(crate) fn clear_language_server_diagnostics(&mut self, server_id: LanguageServerId) {
        self.diagnostics.retain(|_, diagnostics| {
            diagnostics.retain(|(_, provider)| provider.language_server_id() != Some(server_id));
            !diagnostics.is_empty()
        });
        for doc in self.documents_mut() {
            doc.clear_diagnostics_for_language_server(server_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::diagnostic::{DiagnosticTag, NumberOrString, Range, Severity};
    use lsp_client::OffsetEncoding;
    use slotmap::Key;

    #[test]
    fn utf16_diagnostics_preserve_provider_and_protocol_metadata() {
        let text = Rope::from("😀 word\n");
        let diagnostic = lsp::Diagnostic {
            range: lsp::Range::new(lsp::Position::new(0, 3), lsp::Position::new(0, 7)),
            severity: Some(lsp::DiagnosticSeverity::WARNING),
            code: Some(lsp::NumberOrString::String("code".into())),
            source: Some("analyzer".into()),
            message: "message".into(),
            tags: Some(vec![lsp::DiagnosticTag::DEPRECATED]),
            data: Some(serde_json::json!({"id": 1})),
            ..Default::default()
        };
        let provider = DiagnosticProvider::Lsp {
            server_id: LanguageServerId::null(),
            identifier: Some("pull".into()),
        };
        let converted = convert_diagnostic(
            &text,
            None,
            &diagnostic,
            provider.clone(),
            OffsetEncoding::Utf16,
        )
        .unwrap();
        assert_eq!(converted.range, Range { start: 2, end: 6 });
        assert_eq!(converted.provider, provider);
        assert_eq!(converted.severity, Some(Severity::Warning));
        assert_eq!(converted.code, Some(NumberOrString::String("code".into())));
        assert_eq!(converted.source.as_deref(), Some("analyzer"));
        assert_eq!(converted.data, diagnostic.data);
        assert!(matches!(
            converted.tags.as_slice(),
            [DiagnosticTag::Deprecated]
        ));
        assert!(converted.starts_at_word && converted.ends_at_word);
    }

    #[test]
    fn point_diagnostics_respect_the_language_severity_threshold() {
        let text = Rope::from("😀 word\n");
        let config: LanguageConfiguration = toml::from_str(
            "name = 'words'\nscope = 'text.words'\nfile-types = []\nroots = []\ndiagnostic-severity = 'warning'",
        )
        .unwrap();
        let provider = DiagnosticProvider::Lsp {
            server_id: LanguageServerId::null(),
            identifier: None,
        };
        let mut diagnostic = lsp::Diagnostic {
            range: lsp::Range::new(lsp::Position::new(0, 3), lsp::Position::new(0, 3)),
            severity: Some(lsp::DiagnosticSeverity::HINT),
            message: "message".into(),
            ..Default::default()
        };
        assert!(convert_diagnostic(
            &text,
            Some(&config),
            &diagnostic,
            provider.clone(),
            OffsetEncoding::Utf16,
        )
        .is_none());
        diagnostic.severity = Some(lsp::DiagnosticSeverity::WARNING);
        let converted = convert_diagnostic(
            &text,
            Some(&config),
            &diagnostic,
            provider,
            OffsetEncoding::Utf16,
        )
        .unwrap();
        assert!(converted.zero_width);
        assert_eq!(converted.range, Range { start: 2, end: 2 });
        assert!(!converted.starts_at_word && !converted.ends_at_word);
    }
}

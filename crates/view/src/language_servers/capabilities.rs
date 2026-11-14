//! Map configured editor features to negotiated protocol capabilities.

use language_config::LanguageServerFeature;
use lsp_client::lsp;

pub(crate) fn document_selector_method(feature: LanguageServerFeature) -> Option<&'static str> {
    use lsp::request::{self, Request as _};
    Some(match feature {
        LanguageServerFeature::GotoDeclaration => request::GotoDeclaration::METHOD,
        LanguageServerFeature::GotoTypeDefinition => request::GotoTypeDefinition::METHOD,
        LanguageServerFeature::GotoImplementation => request::GotoImplementation::METHOD,
        LanguageServerFeature::DocumentColors => request::DocumentColor::METHOD,
        LanguageServerFeature::FoldingRanges => request::FoldingRangeRequest::METHOD,
        LanguageServerFeature::SelectionRanges => request::SelectionRangeRequest::METHOD,
        LanguageServerFeature::CallHierarchy => request::CallHierarchyPrepare::METHOD,
        LanguageServerFeature::TypeHierarchy => request::TypeHierarchyPrepare::METHOD,
        LanguageServerFeature::SemanticTokens => request::SemanticTokensFullRequest::METHOD,
        LanguageServerFeature::InlayHints => request::InlayHintRequest::METHOD,
        LanguageServerFeature::InlineValues => request::InlineValueRequest::METHOD,
        LanguageServerFeature::LinkedEditing => request::LinkedEditingRange::METHOD,
        LanguageServerFeature::PullDiagnostics => request::DocumentDiagnosticRequest::METHOD,
        _ => return None,
    })
}

/// Whether the server advertises a feature used by Mitos.
/// Push diagnostics have no capability flag and are always permitted.
pub fn supports_feature(
    capabilities: &lsp::ServerCapabilities,
    feature: LanguageServerFeature,
) -> bool {
    use lsp::*;
    match feature {
        LanguageServerFeature::Format => matches!(
            capabilities.document_formatting_provider,
            Some(OneOf::Left(true) | OneOf::Right(_))
        ),
        LanguageServerFeature::GotoDeclaration => matches!(
            capabilities.declaration_provider,
            Some(
                DeclarationCapability::Simple(true)
                    | DeclarationCapability::RegistrationOptions(_)
                    | DeclarationCapability::Options(_),
            )
        ),
        LanguageServerFeature::GotoDefinition => matches!(
            capabilities.definition_provider,
            Some(OneOf::Left(true) | OneOf::Right(_))
        ),
        LanguageServerFeature::GotoTypeDefinition => matches!(
            capabilities.type_definition_provider,
            Some(
                TypeDefinitionProviderCapability::Simple(true)
                    | TypeDefinitionProviderCapability::Options(_)
                    | TypeDefinitionProviderCapability::RegistrationOptions(_),
            )
        ),
        LanguageServerFeature::GotoReference => matches!(
            capabilities.references_provider,
            Some(OneOf::Left(true) | OneOf::Right(_))
        ),
        LanguageServerFeature::GotoImplementation => matches!(
            capabilities.implementation_provider,
            Some(
                ImplementationProviderCapability::Simple(true)
                    | ImplementationProviderCapability::Options(_)
                    | ImplementationProviderCapability::RegistrationOptions(_),
            )
        ),
        LanguageServerFeature::SignatureHelp => capabilities.signature_help_provider.is_some(),
        LanguageServerFeature::Hover => matches!(
            capabilities.hover_provider,
            Some(HoverProviderCapability::Simple(true) | HoverProviderCapability::Options(_),)
        ),
        LanguageServerFeature::DocumentHighlight => matches!(
            capabilities.document_highlight_provider,
            Some(OneOf::Left(true) | OneOf::Right(_))
        ),
        LanguageServerFeature::InlineCompletion => matches!(
            capabilities.inline_completion_provider,
            Some(OneOf::Left(true) | OneOf::Right(_))
        ),
        LanguageServerFeature::Completion => capabilities.completion_provider.is_some(),
        LanguageServerFeature::CodeAction => matches!(
            capabilities.code_action_provider,
            Some(
                CodeActionProviderCapability::Simple(true)
                    | CodeActionProviderCapability::Options(_),
            )
        ),
        LanguageServerFeature::DocumentLinks => capabilities.document_link_provider.is_some(),
        LanguageServerFeature::WorkspaceCommand => capabilities.execute_command_provider.is_some(),
        LanguageServerFeature::DocumentSymbols => matches!(
            capabilities.document_symbol_provider,
            Some(OneOf::Left(true) | OneOf::Right(_))
        ),
        LanguageServerFeature::WorkspaceSymbols => matches!(
            capabilities.workspace_symbol_provider,
            Some(OneOf::Left(true) | OneOf::Right(_))
        ),
        LanguageServerFeature::Diagnostics => true, // there's no extra server capability
        LanguageServerFeature::PullDiagnostics => capabilities.diagnostic_provider.is_some(),
        LanguageServerFeature::RenameSymbol => matches!(
            capabilities.rename_provider,
            Some(OneOf::Left(true)) | Some(OneOf::Right(_))
        ),
        LanguageServerFeature::InlayHints => matches!(
            capabilities.inlay_hint_provider,
            Some(OneOf::Left(true) | OneOf::Right(_))
        ),
        LanguageServerFeature::DocumentColors => matches!(
            capabilities.color_provider,
            Some(
                ColorProviderCapability::Simple(true)
                    | ColorProviderCapability::ColorProvider(_)
                    | ColorProviderCapability::Options(_)
            )
        ),
        LanguageServerFeature::TypeHierarchy => matches!(
            capabilities.type_hierarchy_provider,
            Some(
                TypeHierarchyServerCapabilities::Simple(true)
                    | TypeHierarchyServerCapabilities::Options(_)
                    | TypeHierarchyServerCapabilities::RegistrationOptions(_)
            )
        ),
        LanguageServerFeature::CallHierarchy => matches!(
            capabilities.call_hierarchy_provider,
            Some(
                CallHierarchyServerCapability::Simple(true)
                    | CallHierarchyServerCapability::Options(_)
                    | CallHierarchyServerCapability::RegistrationOptions(_)
            )
        ),
        LanguageServerFeature::SemanticTokens => capabilities
            .semantic_tokens_provider
            .as_ref()
            .is_some_and(|provider| {
                let options = match provider {
                    lsp::SemanticTokensServerCapabilities::SemanticTokensOptions(options) => {
                        options
                    }
                    lsp::SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(
                        options,
                    ) => &options.semantic_tokens_options,
                };
                matches!(options.range, Some(OneOf::Left(true) | OneOf::Right(_)))
                    || matches!(
                        options.full,
                        Some(
                            lsp::SemanticTokensFullOptions::Bool(true)
                                | lsp::SemanticTokensFullOptions::Delta { .. }
                        )
                    )
            }),
        LanguageServerFeature::LinkedEditing => matches!(
            capabilities.linked_editing_range_provider,
            Some(
                LinkedEditingRangeServerCapabilities::Simple(true)
                    | LinkedEditingRangeServerCapabilities::Options(_)
                    | LinkedEditingRangeServerCapabilities::RegistrationOptions(_)
            )
        ),
        LanguageServerFeature::FoldingRanges => matches!(
            capabilities.folding_range_provider,
            Some(
                FoldingRangeProviderCapability::Simple(true)
                    | FoldingRangeProviderCapability::FoldingProvider(_)
                    | FoldingRangeProviderCapability::Options(_)
            )
        ),
        LanguageServerFeature::SelectionRanges => matches!(
            capabilities.selection_range_provider,
            Some(
                SelectionRangeProviderCapability::Simple(true)
                    | SelectionRangeProviderCapability::Options(_)
                    | SelectionRangeProviderCapability::RegistrationOptions(_)
            )
        ),
        LanguageServerFeature::CodeLens => capabilities.code_lens_provider.is_some(),
        LanguageServerFeature::InlineValues => matches!(
            capabilities.inline_value_provider,
            Some(OneOf::Left(true) | OneOf::Right(_))
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_registration_options_enable_the_provider() {
        for (field, feature) in [
            (
                "typeDefinitionProvider",
                LanguageServerFeature::GotoTypeDefinition,
            ),
            (
                "implementationProvider",
                LanguageServerFeature::GotoImplementation,
            ),
        ] {
            let capabilities = serde_json::from_value(serde_json::json!({
                field: {"documentSelector": null, "id": "navigation", "workDoneProgress": true}
            }))
            .unwrap();
            assert!(supports_feature(&capabilities, feature));
        }
    }

    #[test]
    fn advertised_features_respect_boolean_and_option_capabilities() {
        let mut capabilities = lsp::ServerCapabilities::default();
        assert!(!supports_feature(
            &capabilities,
            LanguageServerFeature::RenameSymbol
        ));
        capabilities.rename_provider = Some(lsp::OneOf::Left(false));
        assert!(!supports_feature(
            &capabilities,
            LanguageServerFeature::RenameSymbol
        ));
        capabilities.rename_provider = Some(lsp::OneOf::Left(true));
        assert!(supports_feature(
            &capabilities,
            LanguageServerFeature::RenameSymbol
        ));
        capabilities.rename_provider = Some(lsp::OneOf::Right(lsp::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        }));
        assert!(supports_feature(
            &capabilities,
            LanguageServerFeature::RenameSymbol
        ));
    }

    #[test]
    fn registration_options_and_range_objects_enable_features() {
        let capabilities: lsp::ServerCapabilities = serde_json::from_value(serde_json::json!({
            "implementationProvider": {"documentSelector": null, "id": "implementation"},
            "typeDefinitionProvider": {"documentSelector": null, "id": "types"},
            "callHierarchyProvider": {"documentSelector": null, "id": "calls"},
            "semanticTokensProvider": {"legend": {"tokenTypes": [], "tokenModifiers": []}, "range": {}}
        })).unwrap();
        for feature in [
            LanguageServerFeature::GotoImplementation,
            LanguageServerFeature::GotoTypeDefinition,
            LanguageServerFeature::CallHierarchy,
            LanguageServerFeature::SemanticTokens,
        ] {
            assert!(supports_feature(&capabilities, feature));
        }
    }

    #[test]
    fn push_diagnostics_need_no_advertised_capability() {
        let capabilities = lsp::ServerCapabilities::default();
        assert!(supports_feature(
            &capabilities,
            LanguageServerFeature::Diagnostics
        ));
        assert!(!supports_feature(
            &capabilities,
            LanguageServerFeature::PullDiagnostics
        ));
    }
}

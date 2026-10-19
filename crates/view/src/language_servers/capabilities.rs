//! Map configured editor features to negotiated protocol capabilities.

use language_config::LanguageServerFeature;
use lsp_client::lsp;

pub(crate) fn supports_feature(
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
                    | TypeDefinitionProviderCapability::Options(_),
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
                    | ImplementationProviderCapability::Options(_),
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
            Some(OneOf::Left(true) | OneOf::Right(InlayHintServerCapabilities::Options(_)))
        ),
        LanguageServerFeature::DocumentColors => matches!(
            capabilities.color_provider,
            Some(
                ColorProviderCapability::Simple(true)
                    | ColorProviderCapability::ColorProvider(_)
                    | ColorProviderCapability::Options(_)
            )
        ),
        LanguageServerFeature::CallHierarchy => matches!(
            capabilities.call_hierarchy_provider,
            Some(
                CallHierarchyServerCapability::Simple(true)
                    | CallHierarchyServerCapability::Options(_)
            )
        ),
        LanguageServerFeature::CodeLens => capabilities.code_lens_provider.is_some(),
        LanguageServerFeature::InlineValues => capabilities.inline_value_provider.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

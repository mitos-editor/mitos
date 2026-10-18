//! Typed feature requests and their capability checks.

use super::Client;
use crate::{
    lsp::{self, SignatureHelp},
    Result,
};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashMap, future::Future};

impl Client {
    /// Merge FormattingOptions with 'config.format' and return it
    fn get_merged_formatting_options(
        &self,
        options: lsp::FormattingOptions,
    ) -> lsp::FormattingOptions {
        let config_format = self
            .config
            .as_ref()
            .and_then(|cfg| cfg.get("format"))
            .and_then(|fmt| HashMap::<String, lsp::FormattingProperty>::deserialize(fmt).ok());

        if let Some(mut properties) = config_format {
            // passed in options take precedence over 'config.format'
            properties.extend(options.properties);
            lsp::FormattingOptions {
                properties,
                ..options
            }
        } else {
            options
        }
    }

    pub fn completion(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
        work_done_token: Option<lsp::ProgressToken>,
        context: lsp::CompletionContext,
    ) -> Option<impl Future<Output = Result<Option<lsp::CompletionResponse>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support completion.
        capabilities.completion_provider.as_ref()?;

        let params = lsp::CompletionParams {
            text_document_position: lsp::TextDocumentPositionParams {
                text_document,
                position,
            },
            context: Some(context),
            // TODO: support these tokens by async receiving and updating the choice list
            work_done_progress_params: lsp::WorkDoneProgressParams { work_done_token },
            partial_result_params: lsp::PartialResultParams {
                partial_result_token: None,
            },
        };

        Some(self.call::<lsp::request::Completion>(params))
    }

    pub fn resolve_completion_item(
        &self,
        completion_item: &lsp::CompletionItem,
    ) -> impl Future<Output = Result<lsp::CompletionItem>> + use<> {
        self.call_with_ref::<lsp::request::ResolveCompletionItem>(completion_item)
    }

    pub fn resolve_code_action(
        &self,
        code_action: &lsp::CodeAction,
    ) -> Option<impl Future<Output = Result<lsp::CodeAction>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support resolving code actions.
        match capabilities.code_action_provider {
            Some(lsp::CodeActionProviderCapability::Options(lsp::CodeActionOptions {
                resolve_provider: Some(true),
                ..
            })) => (),
            _ => return None,
        }

        Some(self.call_with_ref::<lsp::request::CodeActionResolveRequest>(code_action))
    }

    pub fn text_document_signature_help(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Option<SignatureHelp>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support signature help.
        capabilities.signature_help_provider.as_ref()?;

        let params = lsp::SignatureHelpParams {
            text_document_position_params: lsp::TextDocumentPositionParams {
                text_document,
                position,
            },
            work_done_progress_params: lsp::WorkDoneProgressParams { work_done_token },
            context: None,
            // lsp::SignatureHelpContext
        };

        Some(self.call::<lsp::request::SignatureHelpRequest>(params))
    }

    pub fn text_document_range_inlay_hints(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        range: lsp::Range,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Option<Vec<lsp::InlayHint>>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        match capabilities.inlay_hint_provider {
            Some(
                lsp::OneOf::Left(true)
                | lsp::OneOf::Right(lsp::InlayHintServerCapabilities::Options(_)),
            ) => (),
            _ => return None,
        }

        let params = lsp::InlayHintParams {
            text_document,
            range,
            work_done_progress_params: lsp::WorkDoneProgressParams { work_done_token },
        };

        Some(self.call::<lsp::request::InlayHintRequest>(params))
    }

    pub fn inline_completion(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
        trigger_kind: lsp::InlineCompletionTriggerKind,
    ) -> Option<impl Future<Output = Result<Option<lsp::InlineCompletionResponse>>> + use<>> {
        if !matches!(
            self.capabilities().inline_completion_provider,
            Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_))
        ) {
            return None;
        }
        Some(
            self.call::<lsp::request::InlineCompletionRequest>(lsp::InlineCompletionParams {
                text_document_position: lsp::TextDocumentPositionParams {
                    text_document,
                    position,
                },
                context: lsp::InlineCompletionContext {
                    trigger_kind,
                    selected_completion_info: None,
                },
                work_done_progress_params: Default::default(),
            }),
        )
    }

    pub fn text_document_document_color(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Vec<lsp::ColorInformation>>> + use<>> {
        self.capabilities.get().unwrap().color_provider.as_ref()?;
        let params = lsp::DocumentColorParams {
            text_document,
            work_done_progress_params: lsp::WorkDoneProgressParams {
                work_done_token: work_done_token.clone(),
            },
            partial_result_params: lsp_types::PartialResultParams {
                partial_result_token: work_done_token,
            },
        };

        Some(self.call::<lsp::request::DocumentColor>(params))
    }

    pub fn text_document_document_link(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Option<Vec<lsp::DocumentLink>>>> + use<>> {
        self.capabilities().document_link_provider.as_ref()?;

        let params = lsp::DocumentLinkParams {
            text_document,
            work_done_progress_params: lsp::WorkDoneProgressParams { work_done_token },
            partial_result_params: lsp::PartialResultParams::default(),
        };

        Some(self.call::<lsp::request::DocumentLinkRequest>(params))
    }

    pub fn resolve_document_link(
        &self,
        params: lsp::DocumentLink,
    ) -> Option<impl Future<Output = Result<lsp::DocumentLink>> + use<>> {
        self.capabilities().document_link_provider.as_ref()?;

        Some(self.call::<lsp::request::DocumentLinkResolve>(params))
    }

    pub fn text_document_hover(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Option<lsp::Hover>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support hover.
        match capabilities.hover_provider {
            Some(
                lsp::HoverProviderCapability::Simple(true)
                | lsp::HoverProviderCapability::Options(_),
            ) => (),
            _ => return None,
        }

        let params = lsp::HoverParams {
            text_document_position_params: lsp::TextDocumentPositionParams {
                text_document,
                position,
            },
            work_done_progress_params: lsp::WorkDoneProgressParams { work_done_token },
            // lsp::SignatureHelpContext
        };

        Some(self.call::<lsp::request::HoverRequest>(params))
    }

    // formatting

    pub fn text_document_formatting(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        options: lsp::FormattingOptions,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Option<Vec<lsp::TextEdit>>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support formatting.
        match capabilities.document_formatting_provider {
            Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_)) => (),
            _ => return None,
        };

        let options = self.get_merged_formatting_options(options);

        let params = lsp::DocumentFormattingParams {
            text_document,
            options,
            work_done_progress_params: lsp::WorkDoneProgressParams { work_done_token },
        };

        Some(self.call::<lsp::request::Formatting>(params))
    }

    pub fn text_document_range_formatting(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        range: lsp::Range,
        options: lsp::FormattingOptions,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Option<Vec<lsp::TextEdit>>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support range formatting.
        match capabilities.document_range_formatting_provider {
            Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_)) => (),
            _ => return None,
        };

        let options = self.get_merged_formatting_options(options);

        let params = lsp::DocumentRangeFormattingParams {
            text_document,
            range,
            options,
            work_done_progress_params: lsp::WorkDoneProgressParams { work_done_token },
        };

        Some(self.call::<lsp::request::RangeFormatting>(params))
    }

    pub fn text_document_diagnostic(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        previous_result_id: Option<String>,
    ) -> Option<impl Future<Output = Result<lsp::DocumentDiagnosticReportResult>> + use<>> {
        let capabilities = self.capabilities();

        // Return early if the server does not support pull diagnostic.
        let identifier = match capabilities.diagnostic_provider.as_ref()? {
            lsp::DiagnosticServerCapabilities::Options(cap) => cap.identifier.clone(),
            lsp::DiagnosticServerCapabilities::RegistrationOptions(cap) => {
                cap.diagnostic_options.identifier.clone()
            }
        };

        let params = lsp::DocumentDiagnosticParams {
            text_document,
            identifier,
            previous_result_id,
            work_done_progress_params: lsp::WorkDoneProgressParams::default(),
            partial_result_params: lsp::PartialResultParams::default(),
        };

        Some(self.call::<lsp::request::DocumentDiagnosticRequest>(params))
    }

    pub fn text_document_document_highlight(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Option<Vec<lsp::DocumentHighlight>>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support document highlight.
        match capabilities.document_highlight_provider {
            Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_)) => (),
            _ => return None,
        }

        let params = lsp::DocumentHighlightParams {
            text_document_position_params: lsp::TextDocumentPositionParams {
                text_document,
                position,
            },
            work_done_progress_params: lsp::WorkDoneProgressParams { work_done_token },
            partial_result_params: lsp::PartialResultParams {
                partial_result_token: None,
            },
        };

        Some(self.call::<lsp::request::DocumentHighlightRequest>(params))
    }

    fn goto_request<
        T: lsp::request::Request<
            Params = lsp::GotoDefinitionParams,
            Result = Option<lsp::GotoDefinitionResponse>,
        >,
    >(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> impl Future<Output = Result<T::Result>> + use<T> {
        let params = lsp::GotoDefinitionParams {
            text_document_position_params: lsp::TextDocumentPositionParams {
                text_document,
                position,
            },
            work_done_progress_params: lsp::WorkDoneProgressParams { work_done_token },
            partial_result_params: lsp::PartialResultParams {
                partial_result_token: None,
            },
        };

        self.call::<T>(params)
    }

    pub fn goto_definition(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Option<lsp::GotoDefinitionResponse>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support goto-definition.
        match capabilities.definition_provider {
            Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_)) => (),
            _ => return None,
        }

        Some(self.goto_request::<lsp::request::GotoDefinition>(
            text_document,
            position,
            work_done_token,
        ))
    }

    pub fn goto_declaration(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Option<lsp::GotoDefinitionResponse>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support goto-declaration.
        match capabilities.declaration_provider {
            Some(
                lsp::DeclarationCapability::Simple(true)
                | lsp::DeclarationCapability::RegistrationOptions(_)
                | lsp::DeclarationCapability::Options(_),
            ) => (),
            _ => return None,
        }

        Some(self.goto_request::<lsp::request::GotoDeclaration>(
            text_document,
            position,
            work_done_token,
        ))
    }

    pub fn goto_type_definition(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Option<lsp::GotoDefinitionResponse>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support goto-type-definition.
        match capabilities.type_definition_provider {
            Some(
                lsp::TypeDefinitionProviderCapability::Simple(true)
                | lsp::TypeDefinitionProviderCapability::Options(_),
            ) => (),
            _ => return None,
        }

        Some(self.goto_request::<lsp::request::GotoTypeDefinition>(
            text_document,
            position,
            work_done_token,
        ))
    }

    pub fn goto_implementation(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Option<lsp::GotoDefinitionResponse>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support goto-definition.
        match capabilities.implementation_provider {
            Some(
                lsp::ImplementationProviderCapability::Simple(true)
                | lsp::ImplementationProviderCapability::Options(_),
            ) => (),
            _ => return None,
        }

        Some(self.goto_request::<lsp::request::GotoImplementation>(
            text_document,
            position,
            work_done_token,
        ))
    }

    pub fn goto_reference(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
        include_declaration: bool,
        work_done_token: Option<lsp::ProgressToken>,
    ) -> Option<impl Future<Output = Result<Option<Vec<lsp::Location>>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support goto-reference.
        match capabilities.references_provider {
            Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_)) => (),
            _ => return None,
        }

        let params = lsp::ReferenceParams {
            text_document_position: lsp::TextDocumentPositionParams {
                text_document,
                position,
            },
            context: lsp::ReferenceContext {
                include_declaration,
            },
            work_done_progress_params: lsp::WorkDoneProgressParams { work_done_token },
            partial_result_params: lsp::PartialResultParams {
                partial_result_token: None,
            },
        };

        Some(self.call::<lsp::request::References>(params))
    }

    pub fn document_symbols(
        &self,
        text_document: lsp::TextDocumentIdentifier,
    ) -> Option<impl Future<Output = Result<Option<lsp::DocumentSymbolResponse>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support document symbols.
        match capabilities.document_symbol_provider {
            Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_)) => (),
            _ => return None,
        }

        let params = lsp::DocumentSymbolParams {
            text_document,
            work_done_progress_params: lsp::WorkDoneProgressParams::default(),
            partial_result_params: lsp::PartialResultParams::default(),
        };

        Some(self.call::<lsp::request::DocumentSymbolRequest>(params))
    }

    pub fn prepare_call_hierarchy(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
    ) -> Option<impl Future<Output = Result<Option<Vec<lsp::CallHierarchyItem>>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        match capabilities.call_hierarchy_provider {
            Some(
                lsp::CallHierarchyServerCapability::Simple(true)
                | lsp::CallHierarchyServerCapability::Options(_),
            ) => (),
            _ => return None,
        }

        let params = lsp::CallHierarchyPrepareParams {
            text_document_position_params: lsp::TextDocumentPositionParams {
                text_document,
                position,
            },
            work_done_progress_params: lsp::WorkDoneProgressParams::default(),
        };

        Some(self.call::<lsp::request::CallHierarchyPrepare>(params))
    }

    pub fn call_hierarchy_incoming(
        &self,
        item: lsp::CallHierarchyItem,
    ) -> Option<impl Future<Output = Result<Option<Vec<lsp::CallHierarchyIncomingCall>>>> + use<>>
    {
        let capabilities = self.capabilities.get().unwrap();

        match capabilities.call_hierarchy_provider {
            Some(
                lsp::CallHierarchyServerCapability::Simple(true)
                | lsp::CallHierarchyServerCapability::Options(_),
            ) => (),
            _ => return None,
        }

        let params = lsp::CallHierarchyIncomingCallsParams {
            item,
            work_done_progress_params: lsp::WorkDoneProgressParams::default(),
            partial_result_params: lsp::PartialResultParams::default(),
        };

        Some(self.call::<lsp::request::CallHierarchyIncomingCalls>(params))
    }

    pub fn call_hierarchy_outgoing(
        &self,
        item: lsp::CallHierarchyItem,
    ) -> Option<impl Future<Output = Result<Option<Vec<lsp::CallHierarchyOutgoingCall>>>> + use<>>
    {
        let capabilities = self.capabilities.get().unwrap();

        match capabilities.call_hierarchy_provider {
            Some(
                lsp::CallHierarchyServerCapability::Simple(true)
                | lsp::CallHierarchyServerCapability::Options(_),
            ) => (),
            _ => return None,
        }

        let params = lsp::CallHierarchyOutgoingCallsParams {
            item,
            work_done_progress_params: lsp::WorkDoneProgressParams::default(),
            partial_result_params: lsp::PartialResultParams::default(),
        };

        Some(self.call::<lsp::request::CallHierarchyOutgoingCalls>(params))
    }

    pub fn prepare_rename(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
    ) -> Option<impl Future<Output = Result<Option<lsp::PrepareRenameResponse>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        match capabilities.rename_provider {
            Some(lsp::OneOf::Right(lsp::RenameOptions {
                prepare_provider: Some(true),
                ..
            })) => (),
            _ => return None,
        }

        let params = lsp::TextDocumentPositionParams {
            text_document,
            position,
        };

        Some(self.call::<lsp::request::PrepareRenameRequest>(params))
    }

    // empty string to get all symbols
    pub fn workspace_symbols(
        &self,
        query: String,
    ) -> Option<impl Future<Output = Result<Option<lsp::WorkspaceSymbolResponse>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support workspace symbols.
        match capabilities.workspace_symbol_provider {
            Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_)) => (),
            _ => return None,
        }

        let params = lsp::WorkspaceSymbolParams {
            query,
            work_done_progress_params: lsp::WorkDoneProgressParams::default(),
            partial_result_params: lsp::PartialResultParams::default(),
        };

        Some(self.call::<lsp::request::WorkspaceSymbolRequest>(params))
    }

    pub fn code_actions(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        range: lsp::Range,
        context: lsp::CodeActionContext,
    ) -> Option<impl Future<Output = Result<Option<Vec<lsp::CodeActionOrCommand>>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support code actions.
        match capabilities.code_action_provider {
            Some(
                lsp::CodeActionProviderCapability::Simple(true)
                | lsp::CodeActionProviderCapability::Options(_),
            ) => (),
            _ => return None,
        }

        let params = lsp::CodeActionParams {
            text_document,
            range,
            context,
            work_done_progress_params: lsp::WorkDoneProgressParams::default(),
            partial_result_params: lsp::PartialResultParams::default(),
        };

        Some(self.call::<lsp::request::CodeActionRequest>(params))
    }

    pub fn rename_symbol(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        position: lsp::Position,
        new_name: String,
    ) -> Option<impl Future<Output = Result<Option<lsp::WorkspaceEdit>>> + use<>> {
        if !matches!(
            self.capabilities().rename_provider,
            Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_))
        ) {
            return None;
        }

        let params = lsp::RenameParams {
            text_document_position: lsp::TextDocumentPositionParams {
                text_document,
                position,
            },
            new_name,
            work_done_progress_params: lsp::WorkDoneProgressParams {
                work_done_token: None,
            },
        };

        Some(self.call::<lsp::request::Rename>(params))
    }

    pub fn code_lens(
        &self,
        text_document: lsp::TextDocumentIdentifier,
    ) -> Option<impl Future<Output = Result<Option<Vec<lsp::CodeLens>>>> + use<>> {
        self.capabilities().code_lens_provider?;
        Some(
            self.call::<lsp::request::CodeLensRequest>(lsp::CodeLensParams {
                text_document,
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            }),
        )
    }

    pub fn resolve_code_lens(
        &self,
        lens: lsp::CodeLens,
    ) -> Option<impl Future<Output = Result<lsp::CodeLens>> + use<>> {
        if self
            .capabilities()
            .code_lens_provider
            .as_ref()?
            .resolve_provider
            != Some(true)
        {
            return None;
        }
        Some(self.call::<lsp::request::CodeLensResolve>(lens))
    }

    pub fn command(
        &self,
        command: lsp::Command,
    ) -> Option<impl Future<Output = Result<Option<Value>>> + use<>> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the language server does not support executing commands.
        capabilities.execute_command_provider.as_ref()?;

        let params = lsp::ExecuteCommandParams {
            command: command.command,
            arguments: command.arguments.unwrap_or_default(),
            work_done_progress_params: lsp::WorkDoneProgressParams {
                work_done_token: None,
            },
        };

        Some(self.call::<lsp::request::ExecuteCommand>(params))
    }
}

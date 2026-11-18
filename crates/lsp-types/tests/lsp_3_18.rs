//! Wire fixtures follow the official LSP 3.18 meta-model:
//! https://github.com/microsoft/language-server-protocol/blob/ba1f1947af19dbf3c33fc763eadd7e7848f80766/_specifications/lsp/3.18/metaModel/metaModel.json

use std::fmt::Debug;

use lsp_types::request::Request;
use lsp_types::*;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

fn round_trip<T: Debug + Serialize + DeserializeOwned>(value: Value) -> T {
    let decoded: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&decoded).unwrap(), value);
    decoded
}

fn range() -> Value {
    json!({"start":{"line":0,"character":0},"end":{"line":0,"character":1}})
}

#[test]
fn standard_client_capabilities_are_available_without_features() {
    round_trip::<ClientCapabilities>(json!({
        "workspace": {
            "textDocumentContent": {"dynamicRegistration": true},
            "foldingRange": {"refreshSupport": true},
            "workspaceEdit": {"metadataSupport": true, "snippetEditSupport": true}
        },
        "textDocument": {
            "filters": {"relativePatternSupport": true},
            "inlineCompletion": {"dynamicRegistration": true},
            "diagnostic": {"markupMessageSupport": true},
            "signatureHelp": {"signatureInformation": {"noActiveParameterSupport": true}},
            "codeAction": {
                "documentationSupport": true,
                "tagSupport": {"valueSet": [1, 42]}
            },
            "codeLens": {"resolveSupport": {"properties": ["command"]}},
            "documentSymbol": {"labelSupport": true}
        },
        "notebookDocument": {"synchronization": {"executionSummarySupport": true}}
    }));
    round_trip::<ClientCapabilities>(json!({}));
}

#[test]
fn inline_completions_support_plain_text_snippets_and_partial_results() {
    round_trip::<<request::InlineCompletionRequest as Request>::Params>(json!({
        "textDocument": {"uri": "file:///tmp/main.rs"},
        "position": {"line": 0, "character": 1},
        "context": {"triggerKind": 2},
        "workDoneToken": "work",
        "partialResultToken": "items"
    }));
    let items = json!([
        {"insertText": "plain"},
        {"insertText": {"kind": "snippet", "value": "${1:name}$0"}, "range": range()}
    ]);
    round_trip::<<request::InlineCompletionRequest as Request>::Result>(items.clone());
    round_trip::<<request::InlineCompletionRequest as Request>::Result>(json!({"items": items}));
    round_trip::<<request::InlineCompletionRequest as Request>::Result>(Value::Null);
}

#[test]
fn document_content_requests_and_registration_use_standard_shapes() {
    round_trip::<<request::TextDocumentContentRequest as Request>::Params>(
        json!({"uri": "generated:document"}),
    );
    round_trip::<<request::TextDocumentContentRequest as Request>::Result>(
        json!({"text": "generated content\n"}),
    );
    round_trip::<<request::TextDocumentContentRefreshRequest as Request>::Params>(
        json!({"uri": "generated:document"}),
    );
    round_trip::<TextDocumentContentRegistrationOptions>(
        json!({"schemes": ["generated"], "id": "content-provider"}),
    );
    for options in [
        json!({"schemes": ["generated"]}),
        json!({"schemes": ["generated"], "id": "content-provider"}),
    ] {
        round_trip::<ServerCapabilities>(json!({"workspace": {"textDocumentContent": options}}));
    }
}

#[test]
fn diagnostic_messages_preserve_plain_text_and_markup() {
    for message in [
        json!("plain"),
        json!({"kind": "markdown", "value": "**error**"}),
    ] {
        round_trip::<Diagnostic>(json!({"range": range(), "message": message}));
    }
    let diagnostic = Diagnostic::new_simple(Range::default(), "plain".to_owned());
    assert_eq!(diagnostic.message.as_str(), "plain");
}

#[test]
fn signature_active_parameters_distinguish_missing_null_and_index() {
    for (field, expected) in [
        (None, None),
        (Some(Value::Null), Some(None)),
        (Some(json!(2)), Some(Some(2))),
    ] {
        let mut help = json!({"signatures": []});
        let mut information = json!({"label": "function(a, b, c)"});
        if let Some(field) = field {
            help["activeParameter"] = field.clone();
            information["activeParameter"] = field;
        }
        assert_eq!(round_trip::<SignatureHelp>(help).active_parameter, expected);
        assert_eq!(
            round_trip::<SignatureInformation>(information).active_parameter,
            expected
        );
    }
}

#[test]
fn code_action_documentation_tags_and_command_tooltips_round_trip() {
    round_trip::<CodeAction>(json!({"title": "Generate", "tags": [1, 42]}));
    round_trip::<CodeActionOptions>(json!({"documentation": [{
        "kind": "refactor.move",
        "command": {"title": "Learn more", "command": "documentation.show", "tooltip": "Open documentation"}
    }]}));
    assert_eq!(CodeActionKind::REFACTOR_MOVE.as_str(), "refactor.move");
    assert_eq!(CodeActionKind::NOTEBOOK.as_str(), "notebook");
    assert_eq!(
        serde_json::to_value(CodeActionTag::LLM_GENERATED).unwrap(),
        json!(1)
    );
}

#[test]
fn workspace_edits_preserve_metadata() {
    round_trip::<ApplyWorkspaceEditParams>(json!({
        "edit": {"changes": {}}, "metadata": {"isRefactoring": true}
    }));
    round_trip::<ApplyWorkspaceEditParams>(json!({"edit": {}}));
}

#[test]
fn document_selectors_support_relative_patterns_and_notebook_cells() {
    let relative = json!({"baseUri": "file:///workspace", "pattern": "**/*.rs"});
    round_trip::<DocumentSelector>(json!([
        {"language": "rust", "pattern": relative},
        {"scheme": "file", "pattern": "**/*.txt"},
        {"notebook": {"notebookType": "jupyter", "pattern": relative}, "language": "python"}
    ]));
    for filter in [
        json!({"notebookType": "jupyter", "pattern": relative}),
        json!({"scheme": "file", "pattern": relative}),
        json!({"pattern": relative}),
    ] {
        round_trip::<NotebookDocumentFilter>(filter);
    }
}

#[test]
fn provider_capabilities_preserve_registration_selectors_ids_and_progress() {
    let registration =
        json!({"documentSelector": null, "id": "provider", "workDoneProgress": true});
    for property in [
        "implementationProvider",
        "typeDefinitionProvider",
        "declarationProvider",
        "typeHierarchyProvider",
        "callHierarchyProvider",
        "selectionRangeProvider",
        "colorProvider",
        "foldingRangeProvider",
        "linkedEditingRangeProvider",
        "inlineValueProvider",
        "inlayHintProvider",
    ] {
        round_trip::<ServerCapabilities>(json!({property: registration}));
        round_trip::<ServerCapabilities>(json!({property: {"workDoneProgress": true}}));
    }
    round_trip::<ServerCapabilities>(json!({"diagnosticProvider": {
        "documentSelector": null, "id": "diagnostics", "workDoneProgress": true,
        "interFileDependencies": true, "workspaceDiagnostics": true
    }}));
    round_trip::<ServerCapabilities>(json!({"semanticTokensProvider": {
        "documentSelector": null, "id": "tokens", "workDoneProgress": true,
        "legend": {"tokenTypes": ["label"], "tokenModifiers": []}
    }}));
    round_trip::<ServerCapabilities>(json!({"notebookDocumentSync": {
        "notebookSelector": [{"notebook": "jupyter"}], "id": "notebooks", "save": true
    }}));
}

#[test]
fn registration_options_preserve_provider_specific_fields() {
    round_trip::<CodeActionRegistrationOptions>(json!({
        "documentSelector": null, "resolveProvider": true, "codeActionKinds": ["quickfix"]
    }));
    round_trip::<CodeLensRegistrationOptions>(json!({
        "documentSelector": null, "resolveProvider": true, "workDoneProgress": true
    }));
    round_trip::<SignatureHelpRegistrationOptions>(json!({
        "documentSelector": null, "triggerCharacters": ["("], "retriggerCharacters": [","],
        "workDoneProgress": true
    }));
    round_trip::<DocumentRangeFormattingRegistrationOptions>(json!({
        "documentSelector": null, "rangesSupport": true
    }));
    assert!(serde_json::from_value::<TextDocumentRegistrationOptions>(json!({})).is_err());
}

#[test]
fn partial_diagnostic_reports_are_progress_and_not_final_results() {
    let partial =
        json!({"relatedDocuments": {"file:///tmp/related": {"kind": "full", "items": []}}});
    round_trip::<DocumentDiagnosticReportProgress>(partial.clone());
    assert!(
        serde_json::from_value::<<request::DocumentDiagnosticRequest as Request>::Result>(partial)
            .is_err()
    );
    round_trip::<<request::DocumentDiagnosticRequest as Request>::Result>(
        json!({"kind": "full", "items": []}),
    );
    round_trip::<ProgressParams>(
        json!({"token": "diagnostics", "value": {"relatedDocuments": {}}}),
    );
    round_trip::<ProgressParams>(json!({"token": 1, "value": [{"label": "partial result"}]}));
    round_trip::<DocumentDiagnosticParams>(json!({"textDocument": {"uri": "file:///tmp/main.rs"}}));
    round_trip::<WorkspaceDiagnosticParams>(json!({"previousResultIds": []}));
}

#[test]
fn new_enumeration_values_and_custom_string_values_are_preserved() {
    assert_eq!(serde_json::to_value(MessageType::DEBUG).unwrap(), json!(5));
    assert_eq!(SemanticTokenType::LABEL.as_str(), "label");
    assert_eq!(LanguageKind::RUST.as_str(), "rust");
    round_trip::<LanguageKind>(json!("custom-language"));
    round_trip::<FoldingRangeKind>(json!("custom-fold"));
}

#[test]
fn semantic_token_range_support_accepts_booleans_and_options() {
    for range in [json!(true), json!(false), json!({})] {
        round_trip::<SemanticTokensClientCapabilitiesRequests>(json!({"range": range}));
        round_trip::<SemanticTokensOptions>(json!({
            "legend": {"tokenTypes": [], "tokenModifiers": []}, "range": range
        }));
    }
}

#[test]
fn optional_fields_are_omitted_and_command_registration_has_one_commands_field() {
    round_trip::<SymbolKindCapability>(json!({}));
    round_trip::<SemanticTokensWorkspaceClientCapabilities>(json!({}));
    round_trip::<FileOperationFilter>(json!({"pattern": {"glob": "**/*.rs"}}));
    let registration = round_trip::<ExecuteCommandRegistrationOptions>(json!({
        "commands": ["example.run"], "workDoneProgress": true
    }));
    let json = serde_json::to_string(&registration).unwrap();
    assert_eq!(json.matches("\"commands\"").count(), 1);
}

#[cfg(feature = "legacy-offset-encoding")]
#[test]
fn legacy_offset_encoding_remains_opt_in() {
    round_trip::<ClientCapabilities>(json!({"offsetEncoding": ["utf-8"]}));
    round_trip::<InitializeResult>(json!({"capabilities": {}, "offsetEncoding": "utf-8"}));
}

use lsp_types::*;
use serde_json::json;

#[test]
fn workspace_diagnostics_serializes_with_the_plural_wire_name() {
    let caps = ClientCapabilities {
        workspace: Some(WorkspaceClientCapabilities {
            diagnostics: Some(DiagnosticWorkspaceClientCapabilities {
                refresh_support: Some(true),
            }),
            ..Default::default()
        }),
        text_document: Some(TextDocumentClientCapabilities {
            diagnostic: Some(DiagnosticClientCapabilities {
                related_document_support: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        serde_json::to_value(caps).unwrap(),
        json!({
            "workspace": {"diagnostics": {"refreshSupport": true}},
            "textDocument": {"diagnostic": {"relatedDocumentSupport": true}}
        })
    );
}

#[test]
fn workspace_diagnostics_deserializes_the_plural_wire_name() {
    let value = json!({"workspace": {"diagnostics": {"refreshSupport": true}}});
    let caps: ClientCapabilities = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        caps.workspace
            .as_ref()
            .unwrap()
            .diagnostics
            .as_ref()
            .unwrap()
            .refresh_support,
        Some(true)
    );
    assert_eq!(serde_json::to_value(caps).unwrap(), value);
}

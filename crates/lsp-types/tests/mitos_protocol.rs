use lsp_types::*;
use serde_json::json;

#[test]
fn multi_range_formatting_capabilities_and_requests_round_trip() {
    let value = json!({"textDocument":{"rangeFormatting":{
        "dynamicRegistration":false,"rangesSupport":true}}});
    let caps: ClientCapabilities = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(caps).unwrap(), value);
    let value = json!({"documentRangeFormattingProvider":{"rangesSupport":true}});
    let caps: ServerCapabilities = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(caps).unwrap(), value);
    let value = json!({"textDocument":{"uri":"file:///tmp/a"},
        "ranges":[{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}],
        "options":{"tabSize":4,"insertSpaces":true}});
    let params: DocumentRangesFormattingParams = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(params).unwrap(), value);
}

#[test]
fn jdtls_workspace_snippets_without_kind_decode_as_snippets() {
    // JDT LS 1.63 emits this shape from resolved Extract to method actions.
    let value = json!({"documentChanges":[{
        "textDocument":{"uri":"file:///tmp/Main.java","version":null},
        "edits":[{"range":{"start":{"line":1,"character":0},
            "end":{"line":2,"character":0}},
            "snippet":{"value":"${1|private,public,protected|} void ${2:method}() {}"}}]
    }]});
    let edit: WorkspaceEdit = serde_json::from_value(value).unwrap();
    let Some(DocumentChanges::Edits(documents)) = edit.document_changes else {
        panic!("document edits")
    };
    let DocumentTextEdit::Snippet(snippet) = &documents[0].edits[0] else {
        panic!("snippet edit")
    };
    assert_eq!(snippet.snippet.kind, "snippet");
    assert_eq!(
        serde_json::to_value(snippet).unwrap()["snippet"]["kind"],
        "snippet"
    );
}

#[test]
fn completion_defaults_and_merge_rules_round_trip() {
    let value = json!({"isIncomplete":false,"items":[{"label":"call","textEditText":"call($1)$0"}],
        "itemDefaults":{"insertTextFormat":2,"data":{"provider":"a"},
        "editRange":{"start":{"line":0,"character":0},"end":{"line":0,"character":2}}},
        "applyKind":{"data":2}});
    let list: CompletionList = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(list.items[0].text_edit_text.as_deref(), Some("call($1)$0"));
    assert_eq!(
        list.apply_kind.as_ref().unwrap().data,
        Some(ApplyKind::MERGE)
    );
    assert_eq!(serde_json::to_value(list).unwrap(), value);
}

#[test]
fn integer_delta_edits_do_not_require_whole_tokens() {
    for data in [json!([3]), json!([]), json!([1, 2, 3, 4, 5, 6])] {
        let value = json!({"start":2,"deleteCount":1,"data":data});
        let edit: SemanticTokensEdit = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(edit).unwrap(), value);
    }
}

#[test]
fn workspace_edits_preserve_snippets_and_all_annotation_ids() {
    let range = json!({"start":{"line":0,"character":0},"end":{"line":0,"character":1}});
    let value = json!({"documentChanges":[
        {"textDocument":{"uri":"file:///tmp/a","version":1},"edits":[
            {"range":range,"newText":"plain"},
            {"range":range,"newText":"annotated","annotationId":"review"},
            {"range":range,"snippet":{"kind":"snippet","value":"${1:name}$0"},"annotationId":"review"}]},
        {"kind":"delete","uri":"file:///tmp/b","annotationId":"review"}],
        "changeAnnotations":{"review":{"label":"Update references","needsConfirmation":true}}});
    let edit: WorkspaceEdit = serde_json::from_value(value.clone()).unwrap();
    let Some(DocumentChanges::Operations(operations)) = &edit.document_changes else {
        panic!("operations")
    };
    let DocumentChangeOperation::Edit(document) = &operations[0] else {
        panic!("document")
    };
    assert!(matches!(&document.edits[0], DocumentTextEdit::Plain(_)));
    assert!(matches!(&document.edits[1], DocumentTextEdit::Annotated(_)));
    assert!(matches!(&document.edits[2], DocumentTextEdit::Snippet(_)));
    assert_eq!(serde_json::to_value(edit).unwrap(), value);
}

#[test]
fn new_capabilities_are_optional_and_use_standard_wire_names() {
    let value = json!({"workspace":{"foldingRange":{"refreshSupport":true},
        "workspaceEdit":{"snippetEditSupport":true}},
        "textDocument":{"completion":{"completionList":{"applyKindSupport":true,
        "itemDefaults":["data","editRange"]}}}});
    let caps: ClientCapabilities = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(caps).unwrap(), value);
    assert_eq!(
        serde_json::to_value(ClientCapabilities::default()).unwrap(),
        json!({})
    );
}

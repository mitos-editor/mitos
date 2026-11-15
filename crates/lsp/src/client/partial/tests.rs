use super::responses::PartialResponse;
use crate::{lsp, Result};
use serde_json::json;
use serde_json::Value;

fn accumulate<T: PartialResponse>(batches: Vec<Value>, final_result: Value) -> Result<T> {
    let mut state = T::State::default();
    for batch in batches {
        T::append(&mut state, T::decode_partial(batch)?)?;
    }
    T::finish(state, T::decode_response(final_result)?)
}

#[test]
fn completion_list_defaults_apply_to_items_in_later_batches() {
    let response = accumulate::<Option<lsp::CompletionResponse>>(
        vec![
            json!({"isIncomplete":true,"items":[{"label":"first"}],"itemDefaults":{
            "commitCharacters":["."],"data":{"provider":7},"insertTextFormat":2,
            "editRange":{"start":{"line":0,"character":1},"end":{"line":0,"character":2}}
        },"applyKind":{"commitCharacters":2,"data":2}}),
            json!([{"label":"second","commitCharacters":[";"],"data":{"item":2}}]),
        ],
        Value::Null,
    )
    .unwrap()
    .unwrap();
    let lsp::CompletionResponse::List(list) = crate::completion::normalize_response(response)
    else {
        panic!()
    };
    assert!(list.is_incomplete);
    assert_eq!(list.items.len(), 2);
    let second = &list.items[1];
    assert_eq!(second.commit_characters.as_ref().unwrap(), &[".", ";"]);
    assert_eq!(second.data, Some(json!({"provider":7,"item":2})));
    assert_eq!(
        second.insert_text_format,
        Some(lsp::InsertTextFormat::SNIPPET)
    );
    assert_eq!(
        serde_json::to_value(&second.text_edit).unwrap(),
        json!({
            "range":{"start":{"line":0,"character":1},"end":{"line":0,"character":2}},"newText":"second"
        })
    );
}

#[test]
fn navigation_links_and_document_symbol_children_are_retained() {
    let range = json!({"start":{"line":0,"character":0},"end":{"line":1,"character":0}});
    let link = json!({"targetUri":"file:///target.rs","targetRange":range,"targetSelectionRange":range,"originSelectionRange":range});
    let response = accumulate::<Option<lsp::GotoDefinitionResponse>>(
        vec![json!([]), json!([link]), json!([link])],
        json!([]),
    )
    .unwrap()
    .unwrap();
    let lsp::GotoDefinitionResponse::Link(links) = response else {
        panic!()
    };
    assert_eq!(serde_json::to_value(links).unwrap(), json!([link, link]));
    let child = json!({"name":"child","kind":12,"range":range,"selectionRange":range});
    let symbols = json!([{"name":"parent","kind":12,"range":range,"selectionRange":range,"children":[child]}]);
    let response =
        accumulate::<Option<lsp::DocumentSymbolResponse>>(vec![symbols.clone()], json!([]))
            .unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), symbols);
    let scalar = json!({"uri":"file:///target.rs","range":range});
    let response =
        accumulate::<Option<lsp::GotoDefinitionResponse>>(vec![], scalar.clone()).unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), scalar);
}

#[test]
fn semantic_data_edits_and_final_result_ids_have_distinct_accumulators() {
    let response = accumulate::<Option<lsp::SemanticTokensResult>>(
        vec![json!({"data":[0,1,2,0,0]}), json!({"data":[0,3,1,0,0]})],
        json!({"data":[],"resultId":"full"}),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(response).unwrap(),
        json!({"data":[0,1,2,0,0,0,3,1,0,0],"resultId":"full"})
    );
    let response = accumulate::<Option<lsp::SemanticTokensFullDeltaResult>>(
        vec![
            json!({"edits":[{"start":1,"deleteCount":2,"data":[8,9]}]}),
            json!({"edits":[{"start":7,"deleteCount":1}]}),
        ],
        json!({"edits":[],"resultId":"delta"}),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(response).unwrap(),
        json!({"resultId":"delta","edits":[
            {"start":1,"deleteCount":2,"data":[8,9]}, {"start":7,"deleteCount":1}
        ]})
    );
    assert!(accumulate::<Option<lsp::SemanticTokensFullDeltaResult>>(
        vec![json!({"edits":[]}), json!({"data":[0,1,2,0,0]}),],
        Value::Null
    )
    .is_err());
}

#[test]
fn semantic_fragments_can_split_tokens_but_must_form_complete_tokens_at_the_end() {
    let batches = vec![
        json!({"data":[0,1]}),
        json!({"data":[2,0,0,0]}),
        json!({"data":[3,1,0,0]}),
    ];
    let expected = json!({"data":[0,1,2,0,0,0,3,1,0,0]});
    let final_result = json!({"data":[],"resultId":"fragmented"});
    let mut expected_with_id = expected.clone();
    expected_with_id["resultId"] = json!("fragmented");
    let full =
        accumulate::<Option<lsp::SemanticTokensResult>>(batches.clone(), final_result.clone())
            .unwrap();
    assert_eq!(serde_json::to_value(full).unwrap(), expected_with_id);
    let range =
        accumulate::<Option<lsp::SemanticTokensRangeResult>>(batches.clone(), Value::Null).unwrap();
    assert_eq!(serde_json::to_value(range).unwrap(), expected);
    let fallback =
        accumulate::<Option<lsp::SemanticTokensFullDeltaResult>>(batches, final_result).unwrap();
    assert_eq!(serde_json::to_value(fallback).unwrap(), expected_with_id);
    assert!(accumulate::<Option<lsp::SemanticTokensResult>>(
        vec![json!({"data":[0,1]})],
        Value::Null
    )
    .is_err());
}

#[test]
fn diagnostic_progress_retains_primary_report_and_related_documents() {
    let primary = json!({"kind":"unchanged","resultId":"primary"});
    let related = json!({"relatedDocuments":{"file:///related.rs":{"kind":"full","items":[],"resultId":"related"}}});
    let mut expected = primary.clone();
    expected["relatedDocuments"] = related["relatedDocuments"].clone();
    let response = accumulate::<lsp::DocumentDiagnosticReport>(
        vec![primary.clone(), related.clone()],
        Value::Null,
    )
    .unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), expected);
    // Servers using the earlier related-only progress shape may send the
    // primary report in the final response instead.
    let response = accumulate::<lsp::DocumentDiagnosticReport>(vec![related], primary).unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), expected);
    assert!(accumulate::<lsp::DocumentDiagnosticReport>(vec![], Value::Null).is_err());
}

#[test]
fn final_workspace_symbols_preserve_resolve_data_and_legacy_deprecation() {
    let range = json!({"start":{"line":0,"character":0},"end":{"line":1,"character":0}});
    let symbols = json!([
        {"name":"resolved","kind":12,"location":{"uri":"file:///target.rs","range":range},"data":{"opaque":[1,2]},"deprecated":true},
        {"name":"lazy","kind":12,"location":{"uri":"file:///target.rs"},"data":{"opaque":[3,4]}}
    ]);
    let response = accumulate::<Option<lsp::WorkspaceSymbolResponse>>(vec![], symbols)
        .unwrap()
        .unwrap();
    let lsp::WorkspaceSymbolResponse::Nested(symbols) = response else {
        panic!()
    };
    assert_eq!(symbols[0].data, Some(json!({"opaque":[1,2]})));
    assert_eq!(symbols[0].tags, Some(vec![lsp::SymbolTag::DEPRECATED]));
    assert_eq!(symbols[1].data, Some(json!({"opaque":[3,4]})));
}

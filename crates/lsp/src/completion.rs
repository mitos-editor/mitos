//! Expand completion-list defaults before publishing completion items.
//!
//! Defaults are expanded once at the wire boundary so preview, acceptance, and
//! resolution all receive the same complete completion item.

use crate::lsp;
use serde_json::Value;

pub(crate) fn normalize_response(response: lsp::CompletionResponse) -> lsp::CompletionResponse {
    match response {
        lsp::CompletionResponse::Array(items) => lsp::CompletionResponse::Array(items),
        lsp::CompletionResponse::List(list) => {
            let defaults = list.item_defaults.unwrap_or_default();
            let apply = list.apply_kind.unwrap_or_default();
            lsp::CompletionResponse::List(lsp::CompletionList {
                is_incomplete: list.is_incomplete,
                items: list.items.into_iter().map(|item| normalize(item, &defaults, &apply)).collect(),
                ..Default::default()
            })
        }
    }
}

fn normalize(mut item: lsp::CompletionItem, defaults: &lsp::CompletionItemDefaults, apply: &lsp::CompletionItemApplyKinds) -> lsp::CompletionItem {
    if item.text_edit.is_none()
        && let Some(range) = &defaults.edit_range
    {
        let new_text = item.text_edit_text.take().unwrap_or_else(|| item.label.clone());
        item.text_edit = Some(match range {
            lsp::OneOf::Left(range) => lsp::CompletionTextEdit::Edit(lsp::TextEdit {
                range: *range,
                new_text,
            }),
            lsp::OneOf::Right(range) => {
                lsp::CompletionTextEdit::InsertAndReplace(lsp::InsertReplaceEdit {
                    insert: range.insert,
                    replace: range.replace,
                    new_text,
                })
            }
        });
    }
    item.insert_text_format = item.insert_text_format.or(defaults.insert_text_format);
    item.insert_text_mode = item.insert_text_mode.or(defaults.insert_text_mode);
    item.commit_characters = match (item.commit_characters, &defaults.commit_characters) {
        (Some(characters), Some(defaults)) if apply.commit_characters == Some(lsp::ApplyKind::MERGE) => {
            let mut merged = defaults.clone();
            for character in characters {
                if !merged.contains(&character) {
                    merged.push(character);
                }
            }
            Some(merged)
        }
        (None, defaults) => defaults.clone(),
        (characters, _) => characters,
    };
    item.data = match (item.data, &defaults.data) {
        (Some(Value::Object(item)), Some(Value::Object(defaults))) if apply.data == Some(lsp::ApplyKind::MERGE) => {
            let mut merged = defaults.clone();
            merged.extend(item);
            Some(Value::Object(merged))
        }
        (None, defaults) => defaults.clone(),
        (data, _) => data,
    };
    item
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn items(value: Value) -> Vec<lsp::CompletionItem> {
        match normalize_response(serde_json::from_value(value).unwrap()) {
            lsp::CompletionResponse::Array(items) => items,
            lsp::CompletionResponse::List(list) => list.items,
        }
    }

    #[test]
    fn defaults_expand_edit_ranges_snippets_and_resolve_data() {
        let range = json!({"start":{"line":0,"character":1},"end":{"line":0,"character":3}});
        let result = items(json!({
            "isIncomplete": true,
            "itemDefaults": {"editRange": range, "insertTextFormat": 2,
                "insertTextMode": 1, "data": {"provider": "a"}},
            "items": [{"label": "call", "textEditText": "call(${1:arg})$0"},
                {"label": "label", "insertText": "ignored for default range"}]
        }));
        assert_eq!(result[0].insert_text_format, Some(lsp::InsertTextFormat::SNIPPET));
        assert_eq!(result[0].insert_text_mode, Some(lsp::InsertTextMode::AS_IS));
        assert_eq!(result[0].data, Some(json!({"provider":"a"})));
        assert_eq!(serde_json::to_value(&result[0].text_edit).unwrap(),
            json!({"range":range,"newText":"call(${1:arg})$0"}));
        assert_eq!(serde_json::to_value(&result[1].text_edit).unwrap()["newText"], "label");
    }

    #[test]
    fn explicit_values_replace_defaults_including_empty_arrays_and_objects() {
        let result = items(json!({"isIncomplete":false,
            "itemDefaults":{"data":{"shared":true},"commitCharacters":["."]},
            "items":[{"label":"a","data":{},"commitCharacters":[]},
                {"label":"b","data":null,"commitCharacters":null}]}));
        assert_eq!(result[0].data, Some(json!({})));
        assert_eq!(result[0].commit_characters, Some(vec![]));
        assert_eq!(result[1].data, Some(json!({"shared":true})));
        assert_eq!(result[1].commit_characters, Some(vec![".".into()]));
    }

    #[test]
    fn merge_is_shallow_and_commit_characters_form_a_union() {
        let result = items(json!({"isIncomplete":false,
            "itemDefaults":{"data":{"shared":true,"nested":{"old":1}},"commitCharacters":[".",";"]},
            "applyKind":{"data":2,"commitCharacters":2},
            "items":[{"label":"a","data":{"nested":{"new":2}},"commitCharacters":[";","("]}]}));
        assert_eq!(result[0].data, Some(json!({"shared":true,"nested":{"new":2}})));
        assert_eq!(result[0].commit_characters, Some(vec![".".into(),";".into(),"(".into()]));
    }

    #[test]
    fn insert_replace_defaults_preserve_both_ranges_and_explicit_edits_win() {
        let insert = json!({"start":{"line":0,"character":0},"end":{"line":0,"character":1}});
        let replace = json!({"start":{"line":0,"character":0},"end":{"line":0,"character":4}});
        let explicit = json!({"range":insert,"newText":"explicit"});
        let result = items(json!({"isIncomplete":false,
            "itemDefaults":{"editRange":{"insert":insert,"replace":replace}},
            "items":[{"label":"a"},{"label":"b","textEdit":explicit}]}));
        assert_eq!(serde_json::to_value(&result[0].text_edit).unwrap(),
            json!({"insert":insert,"replace":replace,"newText":"a"}));
        assert_eq!(serde_json::to_value(&result[1].text_edit).unwrap(),explicit);
    }

    #[test]
    fn array_responses_and_lists_without_defaults_are_unchanged() {
        let item = json!({"label":"a","data":[1,2],"textEditText":"unused"});
        assert_eq!(items(json!([item.clone()])),items(json!({"isIncomplete":false,"items":[item]})));
    }
}

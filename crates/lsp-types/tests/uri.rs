use std::collections::HashMap;

use lsp_types::Uri;
use serde_json::json;

#[test]
fn uri_round_trips_preserve_spelling_and_relative_references() {
    for text in [
        "file:///tmp/My%20File.rs",
        "FILE:///tmp/%61.rs?query=%2f#L1",
        "untitled:Untitled-1",
        "../src/main.rs",
        "",
    ] {
        let parsed: Uri = text.parse().unwrap();
        let deserialized: Uri = serde_json::from_value(json!(text)).unwrap();
        assert_eq!(parsed.as_str(), text);
        assert_eq!(deserialized, parsed);
        assert_eq!(serde_json::to_value(deserialized).unwrap(), json!(text));
    }
}

#[test]
fn invalid_uris_report_parse_errors_during_deserialization() {
    for text in [
        "file:///tmp/bad%GG",
        "file:///tmp/has space",
        "http://[invalid]",
    ] {
        let parse_error = text.parse::<Uri>().unwrap_err();
        let deserialize_error = serde_json::from_value::<Uri>(json!(text)).unwrap_err();
        assert_eq!(deserialize_error.to_string(), parse_error.to_string());
    }
}

#[test]
fn uri_map_keys_and_ordering_use_original_spelling() {
    let value = json!({
        "FILE:///tmp/a.rs": 1,
        "file:///tmp/%61.rs": 2,
        "file:///tmp/a.rs": 3,
    });
    let map: HashMap<Uri, u8> = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(map.len(), 3);
    assert_eq!(serde_json::to_value(&map).unwrap(), value);

    let mut keys: Vec<_> = map.keys().cloned().collect();
    keys.sort();
    let keys: Vec<_> = keys.iter().map(|uri| uri.as_str()).collect();
    assert_eq!(
        keys,
        ["FILE:///tmp/a.rs", "file:///tmp/%61.rs", "file:///tmp/a.rs"]
    );
}

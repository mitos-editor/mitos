//! Define forms as JSON so tests exercise the wire independently from client types.
use serde_json::{json, Value};

pub fn handle(method: &str, params: &Value, args: &[String]) -> Value {
    match method {
        "initialize" => {
            let mut caps = json!({"textDocumentSync":1, "executeCommandProvider":{"commands":["fixture.interactive"]}});
            if !args.iter().any(|arg| arg == "--no-interactive") {
                if args.iter().any(|arg| arg == "--standard") {
                    caps["interactiveResolveProvider"] = json!({"kinds":["command"]});
                } else {
                    caps["experimental"] =
                        json!({"interactiveResolveProvider":{"kinds":["command"]}});
                }
            }
            json!({"capabilities":caps})
        }
        "interactive/listEnum" => json!([{"value":"pkg.Type", "description":"Readable interface"}]),
        "command/resolve" => {
            let mut result = params.clone();
            let scenario = params["arguments"][0].as_str().unwrap_or("ready");
            let answers = params["formAnswers"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let mut fields = match scenario {
                "rounds" => vec![
                    json!({"id":"name", "description":"Name", "type":{"kind":"string"}, "required":true, "default":"seed"}),
                    json!({"id":"case", "description":"Case", "type":{"kind":"enum", "entries":[
                        {"value":"camel", "description":"Camel case"}, {"value":"snake", "description":"Snake case"}]}, "required":true, "default":"snake"}),
                ],
                "all" => vec![
                    json!({"id":"text", "description":"Text", "type":{"kind":"string"}, "required":true, "default":"hello"}),
                    json!({"id":"optional", "description":"Optional", "type":{"kind":"string"}, "required":false}),
                    json!({"id":"bool", "description":"Boolean", "type":{"kind":"bool"}, "required":true, "default":false}),
                    json!({"id":"number", "description":"Number", "type":{"kind":"number"}, "required":true, "default":2.5}),
                    json!({"id":"enum", "description":"Enum", "type":{"kind":"enum", "entries":[{"value":"wire", "description":"Human label"}]}, "required":true}),
                    json!({"id":"file", "description":"File", "type":{"kind":"file", "existence":1, "type":1,"filters":["go"]}, "required":true}),
                    json!({"id":"list", "description":"Nested list", "type":{"kind":"list", "elementType":{"kind":"list", "elementType":{"kind":"number"}}}, "required":true}),
                ],
                "search" => vec![
                    json!({"id":"interface", "description":"Interface", "required":true, "default":"default.Interface",
                    "type":{"kind":"lazyEnum","source":"symbols", "config":{"opaque":[1,false,"test"]}}}),
                ],
                "list" => vec![
                    json!({"id":"list","description":"List","required":true,"default":["a","b"],
                    "type":{"kind":"list","elementType":{"kind":"string"}}}),
                ],
                "unsupported" => vec![
                    json!({"id":"x", "description":"Unknown", "required":true,"type":{"kind":"future"}}),
                ],
                "duplicate" => vec![
                    json!({"id":"x", "description":"One", "required":true,"type":{"kind":"string"}}),
                    json!({"id":"x", "description":"Two", "required":true,"type":{"kind":"string"}}),
                ],
                "ready" => vec![],
                _ => vec![
                    json!({"id":"text", "description":"Text", "type":{"kind":"string"}, "required":true}),
                ],
            };
            if scenario == "rounds" && !answers.is_empty() {
                if answers
                    .iter()
                    .any(|answer| answer["id"] == "name" && answer["value"] == "bad")
                {
                    fields[0]["error"] = json!("Choose another name");
                } else {
                    result["arguments"] = json!(["confirmed", {"answers":answers}]);
                    result["command"] = json!("fixture.resolved");
                    result["formAnswers"] = json!([]);
                    fields = vec![
                        json!({"id":"confirm", "description":"Confirm", "type":{"kind":"bool"}, "required":true}),
                    ];
                }
            } else if !answers.is_empty() {
                fields.clear();
            }
            result["formFields"] = json!(fields);
            result
        }
        _ => Value::Null,
    }
}

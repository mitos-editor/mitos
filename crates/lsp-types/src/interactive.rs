//! Experimental interactive command resolution (golang/go#76331).
//!
//! Keep this extension separate from standard LSP types while the protocol evolves.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{request::Request, ExecuteCommandParams};

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractiveParams {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub form_fields: Vec<FormField>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub form_answers: Vec<FormAnswer>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct FormField {
    pub id: String,
    pub description: String,
    #[serde(rename = "type")]
    pub field_type: FormFieldType,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FormFieldType {
    String,
    Bool,
    Number,
    File {
        /// Bit mask: new = 1, existing = 2. Omission allows both.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        existence: Option<u32>,
        /// Bit mask: regular = 1, directory = 2. Omission means regular.
        #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
        file_type: Option<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        filters: Vec<String>,
    },
    Enum {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        entries: Vec<FormEnumEntry>,
    },
    LazyEnum {
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config: Option<Value>,
    },
    List {
        #[serde(rename = "elementType")]
        element_type: Box<FormFieldType>,
    },
    /// Unknown kinds must not accidentally execute a command with mistyped answers.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct FormAnswer {
    pub id: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct FormEnumEntry {
    pub value: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractiveResolveClientCapabilities {
    pub input_types: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct InteractiveResolveOptions {
    #[serde(default)]
    pub kinds: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct InteractiveListEnumParams {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    pub query: String,
}

#[derive(Debug)]
pub enum ResolveCommand {}
impl Request for ResolveCommand {
    type Params = ExecuteCommandParams;
    type Result = ExecuteCommandParams;
    const METHOD: &'static str = "command/resolve";
}

#[derive(Debug)]
pub enum InteractiveListEnum {}
impl Request for InteractiveListEnum {
    type Params = InteractiveListEnumParams;
    type Result = Vec<FormEnumEntry>;
    const METHOD: &'static str = "interactive/listEnum";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn command_roundtrip_preserves_typed_answers_and_arguments() {
        let value = json!({"command": "test", "arguments": [{"opaque": [1, false]}],
            "formFields": [{"id": "files", "description": "Files", "required": false,
                "type": {"kind": "list", "elementType": {"kind": "file", "existence": 3,
                    "type": 1, "filters": ["go"]}}}],
            "formAnswers": [{"id": "files", "value": null}, {"id": "bool", "value": false},
                {"id": "number", "value": 1.5}, {"id": "list", "value": ["x", "y"]}]});
        let params: ExecuteCommandParams = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(params).unwrap(), value);
    }

    #[test]
    fn ordinary_commands_do_not_gain_extension_fields() {
        let value = json!({"command": "test", "arguments": []});
        let params: ExecuteCommandParams = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(params).unwrap(), value);
    }

    #[test]
    fn unknown_input_kind_is_detectable() {
        let kind: FormFieldType = serde_json::from_value(json!({"kind": "future"})).unwrap();
        assert_eq!(kind, FormFieldType::Unknown);
    }
}

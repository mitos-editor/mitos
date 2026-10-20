//! Protocol conversion, runtime resolution, and bounded display labels.

use std::{collections::BTreeMap, ops::Range};
use stdx::rope::RopeExt as _;

use dap::{requests, RequestSender};
use editor_core::{
    unicode::{segmentation::UnicodeSegmentation, width::UnicodeWidthStr},
    Rope,
};
use lsp_client::{lsp, OffsetEncoding};

use super::{DebuggerInlineValue, MAX_SOURCE_LENGTH};
use crate::debugger::DebuggerVariable;

const MAX_VALUES: usize = 100;
const MAX_LABEL_WIDTH: usize = 150;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Candidate {
    line: usize,
    value: Value,
}

#[derive(Debug, PartialEq, Eq)]
enum Value {
    Text(String),
    Variable { name: String, case_sensitive: bool },
    Expression(String),
}

impl Candidate {
    pub(super) fn variable(line: usize, name: String) -> Self {
        Self {
            line,
            value: Value::Variable {
                name,
                case_sensitive: true,
            },
        }
    }

    pub(super) fn needs_locals(&self) -> bool {
        matches!(self.value, Value::Variable { .. })
    }
}

pub(super) fn from_lsp(
    text: &Rope,
    encoding: OffsetEncoding,
    lines: Range<usize>,
    values: Vec<lsp::InlineValue>,
) -> Vec<Candidate> {
    values
        .into_iter()
        .filter_map(|value| {
            let range = match &value {
                lsp::InlineValue::Text(value) => value.range,
                lsp::InlineValue::VariableLookup(value) => value.range,
                lsp::InlineValue::EvaluatableExpression(value) => value.range,
            };
            if range.start > range.end || !lines.contains(&(range.start.line as usize)) {
                return None;
            }
            let range = lsp_client::util::lsp_range_to_range(text, range, encoding)?;
            let source = || {
                (range.len() <= MAX_SOURCE_LENGTH)
                    .then(|| text.slice_chars(range.from()..range.to()).to_string())
            };
            let value = match value {
                lsp::InlineValue::Text(value) => Value::Text(value.text),
                lsp::InlineValue::VariableLookup(value) => Value::Variable {
                    name: value.variable_name.or_else(source)?,
                    case_sensitive: value.case_sensitive_lookup,
                },
                lsp::InlineValue::EvaluatableExpression(value) => {
                    Value::Expression(value.expression.or_else(source)?)
                }
            };
            Some(Candidate {
                line: text.char_to_line_idx(range.from()),
                value,
            })
        })
        .take(MAX_VALUES)
        .collect()
}

pub(super) async fn resolve(
    candidates: Vec<Candidate>,
    locals: &[DebuggerVariable],
    sender: RequestSender,
    frame: Option<usize>,
    can_evaluate: bool,
) -> Vec<DebuggerInlineValue> {
    let mut lines: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut evaluations = BTreeMap::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    for candidate in candidates {
        let label = match candidate.value {
            Value::Text(text) => text,
            Value::Variable {
                name,
                case_sensitive,
            } => {
                let mut matches = locals.iter().filter(|local| {
                    if case_sensitive {
                        local.name == name
                    } else {
                        local.name.to_lowercase() == name.to_lowercase()
                    }
                });
                let Some(local) = matches.next() else {
                    continue;
                };
                // Ambiguous case-insensitive names cannot be resolved reliably.
                if !case_sensitive && matches.next().is_some() {
                    continue;
                }
                let Some(value) = &local.value else { continue };
                format!("{} = {}", single_line(&name), single_line(value))
            }
            Value::Expression(expression) => {
                if !can_evaluate
                    || expression.trim().is_empty()
                    || expression.chars().take(MAX_SOURCE_LENGTH + 1).count() > MAX_SOURCE_LENGTH
                {
                    continue;
                }
                let value = if let Some(value) = evaluations.get(&expression) {
                    value
                } else {
                    if tokio::time::Instant::now() >= deadline {
                        continue;
                    }
                    let request =
                        sender.request::<requests::Evaluate>(requests::EvaluateArguments {
                            expression: expression.clone(),
                            frame_id: frame,
                            context: Some("hover".into()),
                            format: None,
                        });
                    let result = tokio::time::timeout_at(deadline, request)
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.result);
                    evaluations.entry(expression.clone()).or_insert(result)
                };
                let Some(value) = value else { continue };
                format!("{} = {}", single_line(&expression), single_line(value))
            }
        };
        let label = single_line(&label);
        if !label.trim().is_empty() {
            let labels = lines.entry(candidate.line).or_default();
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
    }
    lines
        .into_iter()
        .map(|(line, labels)| DebuggerInlineValue {
            line,
            text: single_line(&labels.join(" · ")),
        })
        .collect()
}

/// Normalize controls before rendering and bound work independently of adapter output length.
fn single_line(text: &str) -> String {
    let mut result = String::new();
    let mut width = 0;
    for grapheme in text.graphemes(true).take(MAX_LABEL_WIDTH) {
        let clean: String = grapheme
            .chars()
            .map(|ch| {
                if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') {
                    ' '
                } else {
                    ch
                }
            })
            .collect();
        if width + clean.width() > MAX_LABEL_WIDTH - 1 {
            result.push('…');
            return result;
        }
        width += clean.width();
        result.push_str(&clean);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp::request::{InlineValueRequest, Request};
    use serde_json::json;

    #[test]
    fn protocol_arrays_preserve_all_inline_value_variants_and_utf16_ranges() {
        let values: <InlineValueRequest as Request>::Result = serde_json::from_value(json!([
            {"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":9}},"text":"literal"},
            {"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":9}},"caseSensitiveLookup":true},
            {"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":9}}},
            {"range":{"start":{"line":5,"character":0},"end":{"line":5,"character":2}},"text":"outside"}
        ])).unwrap();
        let values = from_lsp(
            &Rope::from_str("😀 answer\n"),
            OffsetEncoding::Utf16,
            0..1,
            values.unwrap(),
        );
        assert_eq!(values.len(), 3);
        assert!(matches!(&values[0].value, Value::Text(text) if text == "literal"));
        assert!(
            matches!(&values[1].value, Value::Variable { name, case_sensitive: true } if name == "answer")
        );
        assert!(matches!(&values[2].value, Value::Expression(text) if text == "answer"));
    }

    #[test]
    fn labels_sanitize_controls_and_fit_wide_graphemes() {
        assert_eq!(single_line("a\n\t\u{2028}b"), "a   b");
        let label = single_line(&"界".repeat(200));
        assert!(label.width() <= MAX_LABEL_WIDTH);
        assert!(label.ends_with('…'));
        assert_eq!(single_line("a👩‍💻b"), "a👩‍💻b");
    }

    #[test]
    fn multiline_provider_ranges_anchor_labels_on_their_start_line() {
        let text = Rope::from_str("first\nsecond\n");
        let values = from_lsp(
            &text,
            OffsetEncoding::Utf16,
            0..1,
            vec![lsp::InlineValueText {
                range: lsp::Range::new(lsp::Position::new(0, 0), lsp::Position::new(1, 6)),
                text: "value".into(),
            }
            .into()],
        );
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].line, 0);
    }
}

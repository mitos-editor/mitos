//! Resolve user values without depending on editor state or a particular frontend.
use super::{DebugConfigCompletion, DebugTemplate};
use anyhow::{Context as _, Result};
use serde_json::Value;
use std::path::Path;

impl DebugConfigCompletion {
    /// The frontend completer, when explicitly configured.
    pub fn kind(&self) -> Option<&str> {
        match self {
            Self::Advanced(config) => config.completion.as_deref(),
            Self::Named(_) => None,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Named(name) => name,
            Self::Advanced(config) => config.name.as_deref().or(self.kind()).unwrap_or(""),
        }
    }

    fn default_value(&self) -> Option<&str> {
        match self {
            Self::Advanced(config) => config.default.as_deref(),
            Self::Named(_) => None,
        }
    }
}

impl DebugTemplate {
    /// Fill positional `{0}` placeholders and apply completion defaults. Missing
    /// values are errors; replacement text is never interpreted as a placeholder.
    /// A whole placeholder containing an unsigned integer becomes a JSON number;
    /// embedded substitutions and configured string literals remain strings.
    /// Existing filename/directory inputs are canonicalized relative to `cwd`.
    /// An explicit `cwd` argument takes precedence over the supplied fallback.
    pub fn resolve(&self, params: &[String], cwd: &Path) -> Result<Value> {
        let params: Vec<_> = (0..params.len().max(self.completion.len()))
            .map(|index| {
                let completion = self.completion.get(index);
                let supplied = params.get(index).map(String::as_str);
                let default = completion.and_then(DebugConfigCompletion::default_value);
                let value = supplied
                    .filter(|value| !value.is_empty())
                    .or(default)
                    .or(supplied)
                    .with_context(|| {
                        format!(
                            "Missing debug parameter {} ({})",
                            index + 1,
                            completion.map_or("", DebugConfigCompletion::label)
                        )
                    })?;
                if !value.is_empty()
                    && matches!(
                        completion.and_then(DebugConfigCompletion::kind),
                        Some("filename" | "directory")
                    )
                {
                    return Ok(cwd
                        .join(value)
                        .canonicalize()
                        .ok()
                        .and_then(|path| path.into_os_string().into_string().ok())
                        .unwrap_or_else(|| value.to_owned()));
                }
                Ok(value.to_owned())
            })
            .collect::<Result<_>>()?;
        let mut args = self
            .args
            .iter()
            .map(|(key, value)| Ok((key.clone(), resolve_value(value, &params)?)))
            .collect::<Result<serde_json::Map<_, _>>>()?;
        args.entry("cwd").or_insert(serde_json::to_value(cwd)?);
        Ok(Value::Object(args))
    }
}

fn placeholder_index(value: &str) -> Result<Option<usize>> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(None);
    }
    Ok(Some(value.parse().with_context(|| {
        format!("Invalid debug placeholder {{{value}}}")
    })?))
}

fn parameter(params: &[String], index: usize) -> Result<&str> {
    params.get(index).map(String::as_str).with_context(|| {
        format!(
            "Missing debug parameter {} for {{{index}}}",
            index.saturating_add(1)
        )
    })
}

fn resolve_value(value: &Value, params: &[String]) -> Result<Value> {
    Ok(match value {
        Value::String(value) => {
            if let Some(token) = value
                .strip_prefix('{')
                .and_then(|value| value.strip_suffix('}'))
                && let Some(index) = placeholder_index(token)?
                && let Ok(integer) = parameter(params, index)?.parse::<u64>()
            {
                return Ok(integer.into());
            }
            let mut result = String::with_capacity(value.len());
            let mut rest = value.as_str();
            while let Some(start) = rest.find('{') {
                let after = &rest[start + 1..];
                let Some(end) = after.find('}') else {
                    break;
                };
                if let Some(index) = placeholder_index(&after[..end])? {
                    result.push_str(&rest[..start]);
                    result.push_str(parameter(params, index)?);
                    rest = &after[end + 1..];
                } else {
                    result.push_str(&rest[..start + 1]);
                    rest = after;
                }
            }
            result.push_str(rest);
            Value::String(result)
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| resolve_value(value, params))
                .collect::<Result<_>>()?,
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), resolve_value(value, params)?)))
                .collect::<Result<_>>()?,
        ),
        value => value.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn template(args: Value, completion: Value) -> DebugTemplate {
        serde_json::from_value(
            json!({"name":"example", "request":"launch", "args":args, "completion":completion}),
        )
        .unwrap()
    }

    #[test]
    fn defaults_apply_to_omitted_and_empty_values() {
        let template = template(
            json!({"pid":"{0}"}),
            json!([{ "name":"process", "default":"42" }]),
        );
        let cwd = Path::new("workspace");
        let expected = json!({"pid":42, "cwd":cwd});
        assert_eq!(template.resolve(&[], cwd).unwrap(), expected);
        assert_eq!(template.resolve(&[String::new()], cwd).unwrap(), expected);
        assert_eq!(template.resolve(&["64".into()], cwd).unwrap()["pid"], 64);
    }

    #[test]
    fn nested_substitutions_preserve_literals_and_do_not_expand_inserted_text() {
        let template = template(
            json!({"args":["{0}", "pré-{1}", {"id":"{1}", "literal":"007", "enabled":true}], "expression":"{name}"}),
            json!([]),
        );
        let result = template
            .resolve(&["literal {1}".into(), "42".into()], Path::new("workspace"))
            .unwrap();
        assert_eq!(
            result["args"],
            json!(["literal {1}", "pré-42", {"id":42, "literal":"007", "enabled":true}])
        );
        assert_eq!(result["expression"], "{name}");
    }

    #[test]
    fn missing_values_are_reported_for_prompts_and_placeholders() {
        let prompt = template(json!({"program":"{0}"}), json!(["program"]));
        assert!(prompt
            .resolve(&[], Path::new("workspace"))
            .unwrap_err()
            .to_string()
            .contains("parameter 1 (program)"));
        let placeholder = template(json!({"program":"{1}"}), json!([]));
        assert!(placeholder
            .resolve(&["first".into()], Path::new("workspace"))
            .unwrap_err()
            .to_string()
            .contains("parameter 2"));
    }

    #[test]
    fn paths_use_the_launch_directory_and_explicit_cwd_is_preserved() {
        let template = template(
            json!({"program":"{0}", "cwd":"custom"}),
            json!([{ "completion":"filename" }]),
        );
        let cwd = Path::new(env!("CARGO_MANIFEST_DIR"));
        let result = template.resolve(&["Cargo.toml".into()], cwd).unwrap();
        assert_eq!(
            result["program"],
            json!(cwd.join("Cargo.toml").canonicalize().unwrap())
        );
        assert_eq!(result["cwd"], "custom");
        let result = template
            .resolve(&["not-an-existing-file".into()], cwd)
            .unwrap();
        assert_eq!(result["program"], "not-an-existing-file");
    }
}

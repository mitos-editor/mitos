//! Adapter request failures retain protocol details while displaying readable text.

use serde_json::Value;
use std::fmt;

/// A failed request, including the adapter's unmodified error payload.
#[derive(Debug, Clone)]
pub struct RequestError {
    pub command: String,
    pub request_seq: u64,
    pub message: Option<String>,
    pub body: Option<Value>,
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} request failed: ", self.command)?;
        let error = self.body.as_ref().and_then(|body| body.get("error"));
        if let Some(format) = error
            .and_then(|error| error.get("format"))
            .and_then(Value::as_str)
            .filter(|format| !format.trim().is_empty())
        {
            let variables = error.and_then(|error| error.get("variables"));
            // Substitute once: inserted values may themselves contain braces.
            for part in format.split_inclusive('}') {
                if let Some((prefix, name)) = part
                    .strip_suffix('}')
                    .and_then(|part| part.rsplit_once('{'))
                    && let Some(value) = variables
                        .and_then(|variables| variables.get(name))
                        .and_then(Value::as_str)
                {
                    write!(f, "{prefix}{value}")?;
                } else {
                    f.write_str(part)?;
                }
            }
            Ok(())
        } else {
            f.write_str(
                self.message
                    .as_deref()
                    .filter(|message| !message.trim().is_empty())
                    .unwrap_or("adapter returned no error details"),
            )
        }
    }
}

impl std::error::Error for RequestError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn display_substitutes_once_and_falls_back_for_missing_or_invalid_details() {
        for (message, body, detail) in [
            (Some("notStopped"), None, "notStopped"),
            (
                None,
                Some(
                    json!({"error": {"id": 42, "format": "Cannot read {name}: {reason}", "variables": {"name": "{reason}", "reason": "unavailable"}}}),
                ),
                "Cannot read {reason}: unavailable",
            ),
            (
                Some("short error"),
                Some(json!({"error": {"format": "Unknown {missing}"}})),
                "Unknown {missing}",
            ),
            (
                Some("fallback"),
                Some(json!({"error": {"format": 42}})),
                "fallback",
            ),
            (
                Some("fallback"),
                Some(json!({"error": {"format": " "}})),
                "fallback",
            ),
            (
                None,
                Some(json!({"unexpected": true})),
                "adapter returned no error details",
            ),
            (None, None, "adapter returned no error details"),
        ] {
            let error = RequestError {
                command: "threads".into(),
                request_seq: 1,
                message: message.map(str::to_owned),
                body,
            };
            assert_eq!(
                error.to_string(),
                format!("threads request failed: {detail}")
            );
        }
    }
}

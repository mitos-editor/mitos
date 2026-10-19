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

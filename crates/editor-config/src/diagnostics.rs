use ::diagnostics::Severity;
use serde::{Deserialize, Serialize};

/// Describes the severity level of a [`::diagnostics::Diagnostic`].
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord)]
pub enum DiagnosticFilter {
    Disable,
    Enable(Severity),
}

impl<'de> Deserialize<'de> for DiagnosticFilter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match &*String::deserialize(deserializer)? {
            "disable" => Ok(DiagnosticFilter::Disable),
            "hint" => Ok(DiagnosticFilter::Enable(Severity::Hint)),
            "info" => Ok(DiagnosticFilter::Enable(Severity::Info)),
            "warning" => Ok(DiagnosticFilter::Enable(Severity::Warning)),
            "error" => Ok(DiagnosticFilter::Enable(Severity::Error)),
            variant => Err(serde::de::Error::unknown_variant(
                variant,
                &["disable", "hint", "info", "warning", "error"],
            )),
        }
    }
}

impl Serialize for DiagnosticFilter {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let filter = match self {
            DiagnosticFilter::Disable => "disable",
            DiagnosticFilter::Enable(Severity::Hint) => "hint",
            DiagnosticFilter::Enable(Severity::Info) => "info",
            DiagnosticFilter::Enable(Severity::Warning) => "warning",
            DiagnosticFilter::Enable(Severity::Error) => "error",
        };
        filter.serialize(serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct InlineDiagnosticsConfig {
    pub cursor_line: DiagnosticFilter,
    pub other_lines: DiagnosticFilter,
    pub min_diagnostic_width: u16,
    pub prefix_len: u16,
    pub max_wrap: u16,
    pub max_diagnostics: usize,
}

impl InlineDiagnosticsConfig {
    pub fn disabled(&self) -> bool {
        matches!(
            self,
            Self {
                cursor_line: DiagnosticFilter::Disable,
                other_lines: DiagnosticFilter::Disable,
                ..
            }
        )
    }
}

impl Default for InlineDiagnosticsConfig {
    fn default() -> Self {
        InlineDiagnosticsConfig {
            cursor_line: DiagnosticFilter::Enable(Severity::Warning),
            other_lines: DiagnosticFilter::Disable,
            min_diagnostic_width: 40,
            prefix_len: 1,
            max_wrap: 20,
            max_diagnostics: 10,
        }
    }
}

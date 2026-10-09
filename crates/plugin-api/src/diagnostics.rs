//! Owned, bounded developer diagnostics; no guest references or private settings.

use serde::{Deserialize, Serialize};

use crate::CapabilitySet;

pub const MAX_DIAGNOSTIC_ENTRIES: usize = 128;
pub const MAX_DIAGNOSTIC_MESSAGE_BYTES: usize = 2048;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginStatus {
    Preparing,
    Ready,
    Busy,
    Disabled,
    Failed,
    ShuttingDown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginEngine {
    Component,
    Declarative,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticLevel {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticEntry {
    pub sequence: u64,
    pub level: DiagnosticLevel,
    pub message: String,
}

/// Aggregate counters remain bounded regardless of request count. Timings use
/// microseconds and distinguish queue wait, guest execution and host application.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RequestTimings {
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub queue_last_us: u64,
    pub queue_max_us: u64,
    pub execution_last_us: u64,
    pub execution_max_us: u64,
    pub apply_last_us: u64,
    pub apply_max_us: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginDiagnostics {
    pub plugin: String,
    pub generation: u64,
    pub status: PluginStatus,
    /// Absent until a configured manifest has been successfully inspected.
    pub engine: Option<PluginEngine>,
    pub declared: CapabilitySet,
    pub effective: CapabilitySet,
    pub queued: usize,
    pub timings: RequestTimings,
    pub entries: Vec<DiagnosticEntry>,
}

impl DiagnosticEntry {
    pub fn bounded(sequence: u64, level: DiagnosticLevel, message: &str) -> Self {
        let mut safe = String::new();
        for character in message
            .chars()
            .filter(|character| !character.is_control() || *character == '\n')
        {
            if safe.len() + character.len_utf8() > MAX_DIAGNOSTIC_MESSAGE_BYTES {
                break;
            }
            safe.push(character);
        }
        Self {
            sequence,
            level,
            message: safe,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_strip_terminal_controls_and_truncate_on_utf8_boundaries() {
        let entry = DiagnosticEntry::bounded(
            1,
            DiagnosticLevel::Error,
            &format!("\x1b\r{}", "μ".repeat(MAX_DIAGNOSTIC_MESSAGE_BYTES)),
        );
        assert_eq!(entry.message.len(), MAX_DIAGNOSTIC_MESSAGE_BYTES);
        assert!(entry.message.chars().all(|character| character == 'μ'));
    }
}

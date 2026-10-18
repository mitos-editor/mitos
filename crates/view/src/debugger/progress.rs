//! User-visible progress belongs to an editor session.

use super::DebugSession;
use dap::events;

impl DebugSession {
    pub(crate) fn progress_start(&mut self, event: events::ProgressStartBody) -> String {
        let status = ProgressState::new(event.title, event.message, event.percentage);
        let status_line = status.status_line();
        self.progress.insert(event.progress_id, status);
        status_line
    }

    pub(crate) fn progress_update(&mut self, event: events::ProgressUpdateBody) -> Option<String> {
        let status = self.progress.get_mut(&event.progress_id)?;
        status.update(event.message, event.percentage);
        Some(status.status_line())
    }

    pub(crate) fn progress_end(&mut self, event: events::ProgressEndBody) -> Option<String> {
        let status = self.progress.remove(&event.progress_id)?;
        Some(status.end_status_line(event.message.as_deref()))
    }
}

/// User-visible state for one long-running DAP progress report.
pub(super) struct ProgressState {
    title: String,
    message: Option<String>,
    percentage: Option<u8>,
}

impl ProgressState {
    /// Starts a progress report with the adapter-provided display fields.
    fn new(title: String, message: Option<String>, percentage: Option<u8>) -> Self {
        Self {
            title,
            message,
            percentage,
        }
    }

    /// Applies an incremental progress update.
    ///
    /// DAP omits unchanged fields, so `None` preserves the previous value
    /// rather than clearing it.
    fn update(&mut self, message: Option<String>, percentage: Option<u8>) {
        if let Some(message) = message {
            self.message = Some(message);
        }
        if let Some(percentage) = percentage {
            self.percentage = Some(percentage);
        }
    }

    /// Formats the in-progress status shown by the editor.
    fn status_line(&self) -> String {
        let mut status = format!("Debug: {}", self.title);
        if let Some(message) = self.message.as_deref() {
            status.push_str(" - ");
            status.push_str(message);
        }
        if let Some(percentage) = self.percentage {
            status.push_str(&format!(" ({}%)", percentage));
        }
        status
    }

    /// Formats the final status, preferring the event's final message over the
    /// most recent incremental message.
    fn end_status_line(&self, message: Option<&str>) -> String {
        let mut status = format!("Debug: {}", self.title);
        if let Some(message) = message.or(self.message.as_deref()) {
            status.push_str(" - ");
            status.push_str(message);
        } else {
            status.push_str(" finished");
        }
        status
    }
}

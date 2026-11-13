//! Bounded transcripts that outlive the adapter which produced them.

use std::collections::VecDeque;

use anyhow::{Context, Result};
use dap::DebugAdapterId;

use crate::{editor::Action, features::output::OutputBuffer, DocumentId, Editor};

const RETAINED_SESSIONS: usize = 8;

#[derive(Default)]
pub(super) struct OutputBuffers {
    sessions: VecDeque<SessionOutput>,
}

struct SessionOutput {
    id: DebugAdapterId,
    number: u64,
    name: String,
    buffer: OutputBuffer,
    category: Option<String>,
    line_start: bool,
}

/// A live session or a completed session with a retained transcript.
pub struct DebuggerOutputSession {
    pub number: u64,
    pub name: String,
    pub is_live: bool,
}

impl super::DebuggerState {
    /// Stable display numbers can reopen transcripts after their adapters disconnect.
    pub fn output_sessions(&self) -> Vec<DebuggerOutputSession> {
        let mut sessions: Vec<_> = self
            .sessions()
            .map(|session| DebuggerOutputSession {
                number: session.number(),
                name: session.name().to_owned(),
                is_live: true,
            })
            .chain(
                self.output
                    .sessions
                    .iter()
                    .filter(|output| self.get_session(output.id).is_none())
                    .map(|output| DebuggerOutputSession {
                        number: output.number,
                        name: output.name.clone(),
                        is_live: false,
                    }),
            )
            .collect();
        sessions.sort_unstable_by_key(|session| session.number);
        sessions
    }
}

impl OutputBuffers {
    /// Keep the most recently ended session last, independently of selection order.
    pub(super) fn finish(&mut self, id: DebugAdapterId) {
        if let Some(index) = self.sessions.iter().position(|session| session.id == id) {
            let session = self.sessions.remove(index).unwrap();
            self.sessions.push_back(session);
        }
    }
}

impl SessionOutput {
    fn title(&self) -> String {
        format!("debug {}: {}", self.number, self.name)
    }

    fn format(&mut self, category: Option<&str>, output: &str) -> String {
        let category = category.unwrap_or("console");
        let changed = self.category.as_deref() != Some(category);
        let mut text = String::new();
        if changed && !self.line_start {
            text.push('\n');
            self.line_start = true;
        }
        self.category = Some(category.to_owned());
        for fragment in output.split_inclusive('\n') {
            if self.line_start {
                text.push('[');
                text.push_str(category);
                text.push_str("] ");
            }
            text.push_str(fragment);
            self.line_start = fragment.ends_with('\n');
        }
        text
    }
}

impl Editor {
    /// Open the selected session's searchable transcript, or the most recently
    /// ended session when no debugger is running. Incoming output never changes focus.
    pub fn open_debugger_output(&mut self, action: Action) -> Result<DocumentId> {
        self.prune_debugger_output();
        let id = self
            .debugger
            .get_active_session()
            .map(|session| session.id())
            .or_else(|| {
                self.debugger
                    .output
                    .sessions
                    .back()
                    .map(|session| session.id)
            })
            .context("No debugger output is available")?;
        self.open_debugger_session_output(id, action)
    }

    /// Open a live or retained session by its stable display number.
    pub fn open_debugger_output_for_session(
        &mut self,
        number: u64,
        action: Action,
    ) -> Result<DocumentId> {
        self.prune_debugger_output();
        let id = self
            .debugger
            .sessions()
            .find(|session| session.number() == number)
            .map(|session| session.id())
            .or_else(|| {
                self.debugger
                    .output
                    .sessions
                    .iter()
                    .find(|session| session.number == number)
                    .map(|session| session.id)
            })
            .with_context(|| format!("No debugger output is available for session {number}"))?;
        self.open_debugger_session_output(id, action)
    }

    fn open_debugger_session_output(
        &mut self,
        id: DebugAdapterId,
        action: Action,
    ) -> Result<DocumentId> {
        let index = self
            .ensure_debugger_output(id)
            .context("No debugger output is available")?;
        let mut output = self.debugger.output.sessions.remove(index).unwrap();
        let title = output.title();
        let document = output.buffer.open(self, &title, action);
        self.debugger.output.sessions.insert(index, output);
        Ok(document)
    }

    pub(crate) fn append_debugger_output(
        &mut self,
        id: DebugAdapterId,
        category: Option<&str>,
        output: &str,
    ) -> bool {
        if category == Some("telemetry") || output.is_empty() {
            return false;
        }
        self.prune_debugger_output();
        let Some(index) = self.ensure_debugger_output(id) else {
            return false;
        };
        let mut session = self.debugger.output.sessions.remove(index).unwrap();
        let text = session.format(category, output);
        let changed = session.buffer.append(self, &text);
        self.debugger.output.sessions.insert(index, session);
        changed
    }

    fn ensure_debugger_output(&mut self, id: DebugAdapterId) -> Option<usize> {
        if let Some(index) = self
            .debugger
            .output
            .sessions
            .iter()
            .position(|session| session.id == id)
        {
            return Some(index);
        }
        let session = self.debugger.get_session(id)?;
        let number = session.number();
        let name = session.name().to_owned();
        self.debugger.output.sessions.push_back(SessionOutput {
            id,
            number,
            name,
            buffer: OutputBuffer::default(),
            category: None,
            line_start: true,
        });
        Some(self.debugger.output.sessions.len() - 1)
    }

    /// Release only completed transcripts. Live sessions retain their own buffers.
    pub(super) fn prune_debugger_output(&mut self) {
        let mut ended = self
            .debugger
            .output
            .sessions
            .iter()
            .filter(|output| self.debugger.get_session(output.id).is_none())
            .count();
        while ended > RETAINED_SESSIONS {
            let index = self
                .debugger
                .output
                .sessions
                .iter()
                .position(|output| self.debugger.get_session(output.id).is_none())
                .unwrap();
            let output = self.debugger.output.sessions.remove(index).unwrap();
            if let Some(document) = output.buffer.document() {
                let _ = self.close_document(document, true);
            }
            ended -= 1;
        }
    }

    pub(crate) fn remove_debugger_session(&mut self, id: DebugAdapterId) {
        self.debugger.remove_session(id);
        self.prune_debugger_output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_output_keeps_lines_and_stream_categories() {
        let mut state = super::super::DebuggerState::default();
        let id = state.insert_session("test".into(), None);
        let mut output = SessionOutput {
            id,
            number: 1,
            name: "test".into(),
            buffer: OutputBuffer::default(),
            category: None,
            line_start: true,
        };
        let mut text = output.format(Some("stdout"), "hel");
        text.push_str(&output.format(Some("stdout"), "lo\nnext"));
        text.push_str(&output.format(Some("stderr"), "error\n"));
        text.push_str(&output.format(None, "done\n"));
        assert_eq!(
            text,
            "[stdout] hello\n[stdout] next\n[stderr] error\n[console] done\n"
        );
    }
}

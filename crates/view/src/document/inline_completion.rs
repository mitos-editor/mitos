//! Inline completion cache and document/view lifetime.
use crate::{handlers::inline_completion::InlineCompletionTrigger, ViewId};
use editor_core::{movement::Direction, Range};
use event::TaskController;
use lsp_client::{lsp, LanguageServerId};

pub struct InlineCompletion {
    pub cursor: usize,
    pub range: Range,
    pub text: String,
    /// Preview includes the unchanged suffix after the replacement range.
    pub lines: Vec<String>,
    pub(crate) server: LanguageServerId,
    pub(crate) command: Option<lsp::Command>,
}

#[derive(Default)]
pub(crate) struct InlineCompletions {
    pub(crate) items: Vec<InlineCompletion>,
    pub(crate) index: usize,
    pub(crate) view: Option<ViewId>,
    pub(crate) controller: TaskController,
    pub(crate) trigger: Option<InlineCompletionTrigger>,
}

impl InlineCompletions {
    pub fn current(&self, view: ViewId) -> Option<&InlineCompletion> {
        (self.view == Some(view))
            .then(|| self.items.get(self.index))
            .flatten()
    }

    pub fn clear(&mut self) {
        self.controller.cancel();
        self.items.clear();
        self.index = 0;
        self.view = None;
    }

    pub(crate) fn cycle(&mut self, view: ViewId, direction: Direction) {
        if self.view != Some(view) {
            return;
        }
        if !self.items.is_empty() {
            self.index = match direction {
                Direction::Forward => (self.index + 1) % self.items.len(),
                Direction::Backward => (self.index + self.items.len() - 1) % self.items.len(),
            };
        }
    }
}

impl InlineCompletions {
    pub(crate) fn remove_view(&mut self, doc: crate::DocumentId, view: ViewId) {
        if self.view == Some(view) {
            self.clear();
        }
        if let Some(trigger) = &self.trigger {
            trigger.cancel_view(doc, view);
        }
    }
}

impl super::Document {
    pub fn inline_completion(&self, view: ViewId) -> Option<&InlineCompletion> {
        self.inline_completions.current(view)
    }
}

/// A visual preview row, produced by the same formatter used for document text.
pub struct InlineCompletionRow {
    pub col: usize,
    pub text: String,
}

impl InlineCompletion {
    pub fn layout(
        &self,
        format: &editor_core::doc_formatter::TextFormat,
        col: usize,
    ) -> Vec<InlineCompletionRow> {
        use editor_core::{
            doc_formatter::DocumentFormatter, text_annotations::TextAnnotations, Rope,
        };
        // Padding supplies the actual anchor column to the formatter for tab stops and wrapping.
        // A non-whitespace sentinel prevents this padding from becoming retained indentation.
        let prefix = if col == 0 {
            String::new()
        } else {
            format!("x{}", " ".repeat(col - 1))
        };
        let text = Rope::from_str(&format!("{prefix}{}", self.lines.join("\n")));
        let annotations = TextAnnotations::default();
        let formatter =
            DocumentFormatter::new_at_prev_checkpoint(text.slice(..), format, &annotations, 0);
        let mut rows = Vec::new();
        for grapheme in formatter {
            if grapheme.char_idx < col {
                continue;
            }
            while rows.len() <= grapheme.visual_pos.row {
                rows.push(InlineCompletionRow {
                    col: 0,
                    text: String::new(),
                });
            }
            let row = &mut rows[grapheme.visual_pos.row];
            if row.text.is_empty() {
                row.col = grapheme.visual_pos.col;
            }
            if !grapheme.source.is_eof()
                && !matches!(grapheme.raw, editor_core::graphemes::Grapheme::Newline)
            {
                row.text.push_str(&grapheme.raw.to_string());
            }
        }
        rows
    }
}

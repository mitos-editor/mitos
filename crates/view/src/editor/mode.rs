//! Shared mode transitions and their required feature/session effects.

use editor_core::Range;

use crate::{document::Mode, events::ModeDidChange, features, Document, Editor, View};

impl Editor {
    /// Change editing mode once, invalidating work from the previous mode before
    /// notifying observers. Frontends retain presentation and input replay behavior.
    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode == mode {
            return;
        }
        let old = self.mode;
        self.mode = mode;
        if old == Mode::Insert {
            features::completion::invalidate(self);
        }
        features::inline_completion::mode_changed(self, old);
        features::navigation::mode_changed(self, old);
        let has_document = self
            .tree
            .try_get(self.tree.focus)
            .is_some_and(|view| self.documents.contains_key(&view.doc));
        if mode == Mode::Normal && has_document {
            self.finish_insert_mode();
        }
        features::signature_help::mode_changed(self, old);
        for (view, _) in self.tree.views_mut() {
            view.diagnostics_handler.active = mode != Mode::Insert;
        }
        if mode == Mode::Insert && has_document {
            features::completion::trigger_auto_completion(self, false);
        }
        if old == Mode::Insert {
            self.handlers.auto_save.left_insert_mode();
        }
        event::dispatch(ModeDidChange {
            editor: self,
            old,
            new: mode,
        })
        .report_to(self.status_sender());
        self.redraw().request();
    }

    /// Switch into normal mode, including cursor and indentation restoration.
    pub fn enter_normal_mode(&mut self) {
        self.set_mode(Mode::Normal);
    }

    fn finish_insert_mode(&mut self) {
        use editor_core::graphemes;
        let (view, doc) = current!(self);

        try_restore_indent(doc, view);

        // if leaving append mode, move cursor back by 1
        if doc.restore_cursor {
            let text = doc.text().slice(..);
            let selection = doc.selection(view.id).clone().transform(|range| {
                let mut head = range.to();
                if range.head > range.anchor {
                    head = graphemes::prev_grapheme_boundary(text, head);
                }

                Range::new(range.from(), head)
            });

            doc.set_selection(view.id, selection);
            doc.restore_cursor = false;
        }
    }
}

fn try_restore_indent(doc: &mut Document, view: &mut View) {
    use editor_core::{
        chars::char_is_whitespace,
        line_ending::{line_end_char_index, str_is_line_ending},
        unicode::segmentation::UnicodeSegmentation,
        Operation, Transaction,
    };

    fn inserted_a_new_blank_line(changes: &[Operation], pos: usize, line_end_pos: usize) -> bool {
        if let [Operation::Retain(move_pos), Operation::Insert(inserted_str), Operation::Retain(_)] =
            changes
        {
            let mut graphemes = inserted_str.graphemes(true);
            move_pos + inserted_str.len() == pos
                && graphemes.next().is_some_and(str_is_line_ending)
                && graphemes.all(|g| g.chars().all(char_is_whitespace))
                && pos == line_end_pos // ensure no characters exists after current position
        } else {
            false
        }
    }

    let doc_changes = doc.changes().changes();
    let text = doc.text().slice(..);
    let range = doc.selection(view.id).primary();
    let pos = range.cursor(text);
    let line_end_pos = line_end_char_index(&text, range.cursor_line(text));

    if inserted_a_new_blank_line(doc_changes, pos, line_end_pos) {
        // Removes tailing whitespaces for the primary selection only, preserving existing behavior
        let line_start_pos = text.line_to_char(range.cursor_line(text));
        let transaction =
            Transaction::change(doc.text(), [(line_start_pos, pos, None)].into_iter());
        crate::editing::apply(doc, &transaction, view.id);
    }
}

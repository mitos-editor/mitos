//! Shared mode transitions and their required feature/session effects.

use crate::editing::{
    self,
    insert::{CommentContinuation, LineBoundary, Open},
    EditError,
};
use editor_core::{graphemes, Range, Transaction};
use stdx::rope::RopeExt as _;

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
            .try_get(self.tree.focus())
            .is_some_and(|view| self.documents.contains_key(&view.doc));
        if old == Mode::Insert && has_document {
            self.finish_insert_mode();
        }
        if mode == Mode::Select && has_document {
            self.prepare_select_mode();
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

    /// Use normal editing semantics with debugger command bindings. This does not
    /// start a session and remains active while navigating between source files.
    pub fn enter_debug_mode(&mut self) {
        self.set_mode(Mode::Debug);
    }

    /// Finish a document's editing mode before moving focus, retaining debugger controls.
    pub(super) fn leave_editing_mode(&mut self) {
        if self.mode != Mode::Debug {
            self.enter_normal_mode();
        }
    }

    /// Insert at the start of each selection, retaining its extent while typing.
    pub fn enter_insert_mode(&mut self) {
        let (view, doc) = current!(self);
        doc.set_append_cursor_restoration(view.id, false);
        let selection = doc
            .selection(view.id)
            .clone()
            .transform(|range| Range::new(range.to(), range.from()));
        doc.set_selection(view.id, selection);
        self.set_mode(Mode::Insert);
    }

    /// Append after each selection. Failure to make room at EOF leaves mode and cursors alone.
    pub fn enter_append_mode(&mut self) -> Result<(), EditError> {
        let (view, doc) = current!(self);
        let end = doc.text().len_chars();
        let last = doc
            .selection(view.id)
            .iter()
            .last()
            .expect("selection is nonempty");
        if !last.is_empty() && last.to() == end {
            let transaction = Transaction::change(
                doc.text(),
                [(end, end, Some(doc.line_ending.as_str().into()))].into_iter(),
            );
            editing::apply(doc, &transaction, view.id)?;
        }
        let selection = doc.selection(view.id).clone().transform(|range| {
            Range::new(
                range.from(),
                graphemes::next_grapheme_boundary(doc.text().slice(..), range.to()),
            )
        });
        doc.set_append_cursor_restoration(view.id, true);
        doc.set_selection(view.id, selection);
        self.set_mode(Mode::Insert);
        Ok(())
    }

    /// Select text, expanding an empty EOF cursor over the preceding grapheme.
    pub fn enter_select_mode(&mut self) {
        self.set_mode(Mode::Select);
    }

    fn prepare_select_mode(&mut self) {
        let (view, doc) = current!(self);
        let text = doc.text().slice(..);
        let selection = doc.selection(view.id).clone().transform(|range| {
            if range.is_empty() && range.head == text.len_chars() {
                Range::new(
                    graphemes::prev_grapheme_boundary(text, range.anchor),
                    range.head,
                )
            } else {
                range
            }
        });
        doc.set_selection(view.id, selection);
    }

    /// Move to line boundaries, indenting empty lines before entering insert mode.
    pub fn insert_at_line_boundary(&mut self, boundary: LineBoundary) -> Result<(), EditError> {
        let extend = self.mode() == Mode::Select;
        let (view, doc) = current!(self);
        editing::insert::insert_at_line_boundary(doc, view.id, boundary, extend)?;
        doc.set_append_cursor_restoration(view.id, false);
        self.set_mode(Mode::Insert);
        Ok(())
    }

    /// Open indented lines and enter insert mode only after a successful edit.
    pub fn open_lines(
        &mut self,
        open: Open,
        comments: CommentContinuation,
        count: usize,
    ) -> Result<(), EditError> {
        let (view, doc) = current!(self);
        editing::insert::open_lines(doc, view.id, open, comments, count)?;
        doc.set_append_cursor_restoration(view.id, false);
        self.set_mode(Mode::Insert);
        Ok(())
    }

    fn finish_insert_mode(&mut self) {
        let (view, doc) = current!(self);

        if let Err(error) = try_restore_indent(doc, view) {
            self.set_error(|| error.to_string());
        }
        let (view, doc) = current!(self);

        // if leaving append mode, move cursor back by 1
        if doc.take_append_cursor_restoration(view.id) {
            let text = doc.text().slice(..);
            let selection = doc.selection(view.id).clone().transform(|range| {
                let mut head = range.to();
                if range.head > range.anchor {
                    head = graphemes::prev_grapheme_boundary(text, head);
                }

                Range::new(range.from(), head)
            });

            doc.set_selection(view.id, selection);
        }
    }
}

fn try_restore_indent(doc: &mut Document, view: &mut View) -> Result<(), EditError> {
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
            move_pos + inserted_str.chars().count() == pos
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
        let line_start_pos = text.line_to_char_idx(range.cursor_line(text));
        let transaction =
            Transaction::change(doc.text(), [(line_start_pos, pos, None)].into_iter());
        editing::apply(doc, &transaction, view.id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

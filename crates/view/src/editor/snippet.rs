//! Shared snippet insertion and tabstop navigation.

use crate::{
    document::Mode,
    editing::{self, EditError},
    Editor,
};
use editor_core::movement::Direction;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnippetNavigation {
    Moved,
    Finished,
    AtStart,
    Inactive,
}

impl Editor {
    /// Insert at each cursor and start or nest the rendered snippet session.
    pub fn insert_snippet(&mut self, snippet: &snippets::Snippet) -> Result<(), EditError> {
        let mode = self.mode();
        let mut context = {
            let (view, doc) = current_ref!(self);
            self.snippet_context(doc, doc.text(), doc.selection(view.id), snippet)
        };
        let (view, doc) = current!(self);
        let text = doc.text().slice(..);
        let replace_placeholder = doc.snippet_placeholder_active(view.id);
        let (transaction, selection, rendered) = snippet.render(
            doc.text(),
            doc.selection(view.id),
            |range| {
                if replace_placeholder
                    || mode == Mode::Select
                    || (!range.is_empty() && !range.is_single_grapheme(text))
                {
                    (range.from(), range.to())
                } else {
                    let cursor = range.cursor(text);
                    (cursor, cursor)
                }
            },
            &mut context,
        );
        let selection = rendered.first_selection(Direction::Forward, selection.primary_index());
        editing::apply(doc, &transaction.with_selection(selection), view.id)?;
        doc.install_snippet(view.id, rendered);
        self.set_mode(Mode::Insert);
        Ok(())
    }

    pub fn cycle_snippet_choice(&mut self, direction: Direction) -> Result<bool, EditError> {
        let (view, doc) = current!(self);
        let Some(transaction) = doc.snippet_choice_transaction(view.id, direction) else {
            return Ok(false);
        };
        editing::apply(doc, &transaction, view.id)?;
        Ok(true)
    }

    pub fn move_snippet_tabstop(&mut self, direction: Direction) -> SnippetNavigation {
        let (view, doc) = current!(self);
        doc.move_snippet_tabstop(view.id, direction)
    }
}

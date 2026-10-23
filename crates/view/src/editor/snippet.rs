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
        let (view, doc) = current!(self);
        let text = doc.text().slice(..);
        let (transaction, selection, rendered) = snippet.render(
            doc.text(),
            doc.selection(view.id),
            |range| {
                let cursor = range.cursor(text);
                (cursor, cursor)
            },
            &mut doc.snippet_ctx(doc.text(), doc.selection(view.id), mode),
        );
        let selection = rendered.first_selection(Direction::Forward, selection.primary_index());
        editing::apply(doc, &transaction.with_selection(selection), view.id)?;
        doc.install_snippet(view.id, rendered);
        self.set_mode(Mode::Insert);
        Ok(())
    }

    pub fn move_snippet_tabstop(&mut self, direction: Direction) -> SnippetNavigation {
        let (view, doc) = current!(self);
        doc.move_snippet_tabstop(view.id, direction)
    }
}

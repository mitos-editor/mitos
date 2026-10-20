//! Snippet placeholder edits and invalidation during editor operations.

use crate::{events::DocumentFocusLost, Editor};
use event::register_hook;

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.active_snippet = None;
        }
        Ok(())
    });
}

impl Editor {
    /// Remove the active placeholder before inserting replacement input.
    pub fn clear_snippet_placeholder(&mut self) -> Result<bool, crate::editing::EditError> {
        let (view, doc) = current!(self);
        let Some(snippet) = &doc.active_snippet else {
            return Ok(false);
        };
        crate::editing::apply(doc, &snippet.delete_placeholder(doc.text()), view.id)?;
        Ok(true)
    }
}

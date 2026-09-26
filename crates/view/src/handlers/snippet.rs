//! Snippet range tracking and invalidation during editor operations.

use crate::events::{DocumentDidChange, DocumentFocusLost, SelectionDidChange};
use event::register_hook;

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        if let Some(snippet) = &event.doc.active_snippet
            && !snippet.is_valid(event.doc.selection(event.view))
        {
            event.doc.active_snippet = None;
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if let Some(snippet) = &mut event.doc.active_snippet {
            let invalid = snippet.map(event.changes);
            if invalid {
                event.doc.active_snippet = None;
            }
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        let editor = &mut event.editor;
        doc_mut!(editor).active_snippet = None;
        Ok(())
    });
}

//! Snippet range tracking and invalidation during editor operations.

use crate::events::DocumentFocusLost;
use event::register_hook;

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.active_snippet = None;
        }
        Ok(())
    });
}

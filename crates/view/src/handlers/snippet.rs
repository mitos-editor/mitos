//! Snippet invalidation when the editor changes document focus.

use crate::events::DocumentFocusLost;
use event::register_hook;

pub(super) fn register_hooks(events: &event::EventHub) {
    register_hook!(events => move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.clear_snippet();
        }
        Ok(())
    });
}

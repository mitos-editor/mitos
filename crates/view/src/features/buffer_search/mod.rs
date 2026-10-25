//! Per-view search statistics, bounded workers, and snapshot-checked publication.
mod query;
mod state;
mod worker;

use crate::events::{ConfigDidChange, DocumentFocusLost};
pub use query::Query;
pub(crate) use state::State;
pub use worker::Handler;

pub(crate) fn register_hooks() {
    event::register_hook!(move |event: &mut ConfigDidChange<'_>| {
        if event.old.search != event.new.search
            || event.old.statusline.shows_search_position()
                != event.new.statusline.shows_search_position()
        {
            for doc in event.editor.documents_mut() {
                doc.invalidate_search_counts();
            }
        }
        Ok(())
    });
    event::register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        if event
            .editor
            .tree
            .try_get(event.view)
            .is_none_or(|view| view.document_id() != event.doc)
            && let Some(doc) = event.editor.document_mut(event.doc)
            && let Some(state) = doc.search_state_mut(event.view)
        {
            state.leave_document();
        }
        Ok(())
    });
}

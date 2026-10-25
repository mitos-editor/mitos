//! Buffer search coordination without prompts or status messages.

use crate::features::buffer_search::Query;
use crate::Editor;
use editor_core::{
    movement::{Direction, Movement},
    search,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOutcome {
    Found { wrapped: bool },
    NotFound,
}

impl Editor {
    /// Search repeatedly, updating the primary selection or adding matches. Stop
    /// at the first missing match and center the final cursor once.
    pub fn search_buffer(
        &mut self,
        query: &Query,
        movement: Movement,
        direction: Direction,
        count: usize,
        wrap: bool,
    ) -> SearchOutcome {
        let config = self.config();
        let padding = config.scroll.padding;
        let handler = self.handlers.buffer_search.clone();
        let (view, doc) = current!(self);
        let mut outcome = SearchOutcome::NotFound;
        let mut wrapped = false;
        let mut changed = false;
        for _ in 0..count {
            let selection = doc.selection(view.id);
            let Some(found) = search::find_regex(
                doc.text().slice(..),
                selection.primary(),
                query.regex(),
                direction,
                wrap,
            ) else {
                outcome = SearchOutcome::NotFound;
                break;
            };
            let selection = match movement {
                Movement::Extend => selection.clone().push(found.range),
                Movement::Move => selection
                    .clone()
                    .replace(selection.primary_index(), found.range),
            };
            doc.set_selection(view.id, selection);
            changed = true;
            wrapped |= found.wrapped;
            outcome = SearchOutcome::Found { wrapped };
        }
        if changed {
            view.ensure_cursor_in_view_center(doc, padding);
        }
        if changed && config.statusline.shows_search_position() {
            let range = doc.selection(view.id).primary();
            handler.request(doc, view.id, query, range, config.search.match_limit());
        } else if let Some(state) = doc.search_state_mut(view.id) {
            state.clear_position();
        }
        outcome
    }
}

mod replacement;
pub use replacement::{
    PendingReplacement, ReplacementOutcome, ReplacementPlan, WorkspaceReplacement,
};

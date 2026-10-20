//! Buffer search coordination without prompts or status messages.

use crate::Editor;
use editor_core::{
    movement::{Direction, Movement},
    search,
};
use stdx::rope;

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
        regex: &rope::Regex,
        movement: Movement,
        direction: Direction,
        count: usize,
        wrap: bool,
    ) -> SearchOutcome {
        let padding = self.config().scroll.padding;
        let (view, doc) = current!(self);
        let mut outcome = SearchOutcome::NotFound;
        let mut wrapped = false;
        let mut changed = false;
        for _ in 0..count {
            let selection = doc.selection(view.id);
            let Some(found) = search::find_regex(
                doc.text().slice(..),
                selection.primary(),
                regex,
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
        outcome
    }
}

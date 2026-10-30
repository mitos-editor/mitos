use text_layout::doc_formatter::FormattedGrapheme;
use view::editor::CursorCache;

use crate::ui::document::TextRenderer;

pub use diagnostics::InlineDiagnostics;
pub use text_render::{Decoration, DecorationManager};

pub mod blame;
pub mod code_lenses;
pub mod conflicts;
pub mod debug_values;
mod diagnostics;
mod end_of_line;
pub mod inline_completion;

/// Cursor rendering is done externally so all the cursor decoration
/// does is save the position of primary cursor
pub struct Cursor<'a> {
    pub cache: &'a CursorCache,
    pub primary_cursor: usize,
}
impl Decoration for Cursor<'_> {
    fn reset_pos(&mut self, pos: usize) -> usize {
        if pos <= self.primary_cursor {
            self.primary_cursor
        } else {
            usize::MAX
        }
    }

    fn decorate_grapheme(
        &mut self,
        renderer: &mut TextRenderer,
        grapheme: &FormattedGrapheme,
    ) -> usize {
        if grapheme.is_virtual() {
            return self.primary_cursor;
        }
        if renderer.column_in_bounds(grapheme.visual_pos.col, grapheme.width())
            && renderer.offset.row <= grapheme.visual_pos.row
        {
            let position = grapheme.visual_pos - renderer.offset;
            self.cache.set(Some(position));
        }
        usize::MAX
    }
}

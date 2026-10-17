//! Terminal presentation of shared navigation results.
use view::{features::navigation::NavigationUpdate, Editor};

use crate::compositor::Compositor;

pub fn show(editor: &mut Editor, compositor: &mut Compositor, update: NavigationUpdate) {
    if let Some(locations) = update.resolve(editor) {
        crate::commands::lsp::goto_impl(editor, compositor, locations);
    }
}

//! Terminal presentation of shared navigation results.
use view::{
    features::navigation::{NavigationKind, NavigationUpdate},
    Editor,
};

use crate::compositor::Compositor;

pub fn show(editor: &mut Editor, compositor: &mut Compositor, update: NavigationUpdate) {
    let title = match update.kind() {
        NavigationKind::Declaration => "Declarations",
        NavigationKind::Definition => "Definitions",
        NavigationKind::TypeDefinition => "Type Definitions",
        NavigationKind::Implementation => "Implementations",
        NavigationKind::References | NavigationKind::SelectReferences => "References",
    };
    if let Some(locations) = update.resolve(editor) {
        crate::commands::lsp::goto_impl(title, editor, compositor, locations);
    }
}

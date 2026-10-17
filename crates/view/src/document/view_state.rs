//! State belonging to one document/view relationship.

use crate::view::ViewPosition;
use editor_core::{text_annotations::Overlay, Selection};

#[derive(Debug)]
pub(super) struct DocumentViewState {
    pub selection: Selection,
    pub position: ViewPosition,
    pub jump_labels: Vec<Overlay>,
}

impl DocumentViewState {
    pub(super) fn new(selection: Selection) -> Self {
        Self {
            selection,
            position: ViewPosition::default(),
            jump_labels: Vec::new(),
        }
    }
}

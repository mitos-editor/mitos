//! Backend-independent editor state and behavior.
//!
//! [`Editor`] owns open [`Document`]s, visible [`View`]s, split layout, registers,
//! themes, and protocol clients. A document may be displayed by multiple views;
//! selections and scroll offsets are therefore keyed by [`ViewId`] and belong to
//! the document/view relationship rather than to either object alone.
//!
//! This crate may depend on editing primitives and protocol clients, but not on
//! a concrete terminal renderer. User-interface crates should invoke editor
//! operations here and render the resulting state instead of duplicating model
//! state in widgets.

#[macro_use]
pub mod macros;

pub mod action;
pub mod annotations;
pub mod clipboard;
pub mod config;
pub mod custom_commands;
pub mod document;
pub mod editor;
pub mod events;
pub mod expansion;
pub use ui_core::graphics;
pub mod gutter;
pub mod handlers;
pub mod icons;
pub mod info;
pub mod quicklist;
pub mod register;
pub mod theme;
pub mod tree;
pub mod view;
pub use ui_core::{input, keyboard};

use std::num::NonZeroUsize;

/// Stable identifier assigned to a document owned by an [`Editor`].
///
/// The non-zero representation preserves Rust's niche optimization so
/// `Option<DocumentId>` occupies the same space as `DocumentId`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DocumentId(NonZeroUsize);

impl Default for DocumentId {
    fn default() -> DocumentId {
        DocumentId(NonZeroUsize::new(1).unwrap())
    }
}

#[cfg(test)]
impl DocumentId {
    /// Constructs a `DocumentId` with the given non-zero id, for use in tests
    /// that need several distinct ids without spinning up an `Editor`.
    pub(crate) fn new(id: usize) -> DocumentId {
        DocumentId(NonZeroUsize::new(id).expect("document id must be non-zero"))
    }
}

impl std::fmt::Display for DocumentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}", self.0))
    }
}

slotmap::new_key_type! {
    /// Generational identifier for a view in the editor's split tree.
    pub struct ViewId;
}

/// Vertical placement used when scrolling a cursor into a view.
pub enum Align {
    /// Place the cursor on the first visual row.
    Top,
    /// Place the cursor near the middle visual row.
    Center,
    /// Place the cursor on the last visual row.
    Bottom,
}

/// Scrolls `view` so its primary cursor appears at `align`.
///
/// Alignment is measured in soft-wrapped visual rows, not document lines. The
/// bottom row is reduced by one to account for zero-based visual offsets.
pub fn align_view(doc: &mut Document, view: &View, align: Align) {
    let doc_text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(doc_text);
    let viewport = view.inner_area(doc);
    let last_line_height = viewport.height.saturating_sub(1);
    let mut view_offset = doc.view_offset(view.id);

    let relative = match align {
        Align::Center => last_line_height / 2,
        Align::Top => 0,
        Align::Bottom => last_line_height,
    };

    let text_fmt = doc.text_format(viewport.width, None);
    (view_offset.anchor, view_offset.vertical_offset) = char_idx_at_visual_offset(
        doc_text,
        cursor,
        -(relative as isize),
        0,
        &text_fmt,
        &view.text_annotations(doc, None),
    );
    doc.set_view_offset(view.id, view_offset);
}

pub use document::Document;
pub use editor::Editor;
use editor_core::char_idx_at_visual_offset;
pub use spellbook::Dictionary;
pub use theme::Theme;
pub use view::View;

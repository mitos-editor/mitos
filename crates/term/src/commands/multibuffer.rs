//! Terminal adapters for the shared multi-buffer operations.

use super::Context;
use view::{
    editor::Action,
    multibuffer::{MultiBufferExpand, MultiBufferLocation},
};

pub(super) fn open_selections_in_multibuffer(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let text = doc.text().slice(..);
    let locations: Vec<_> = doc
        .selection(view.id)
        .ranges()
        .iter()
        .map(|range| {
            let start = text.char_to_line(range.from());
            let end = text.char_to_line(range.to().saturating_sub(1).max(range.from()));
            MultiBufferLocation::Document(doc.id(), Some((start, end)))
        })
        .collect();
    if let Err(err) = cx.editor.open_multibuffer(locations, Action::Replace) {
        cx.editor.set_error(|| err.to_string());
    }
}

fn expand(cx: &mut Context, direction: MultiBufferExpand) {
    let view = view!(cx.editor);
    if let Err(err) = cx
        .editor
        .expand_multibuffer_excerpt(view.document_id(), view.id, direction)
    {
        cx.editor.set_error(|| err.to_string());
    }
}
pub(super) fn extend_multibuffer_excerpt(cx: &mut Context) {
    expand(cx, MultiBufferExpand::Both);
}
pub(super) fn expand_multibuffer_excerpt_up(cx: &mut Context) {
    expand(cx, MultiBufferExpand::Up);
}
pub(super) fn expand_multibuffer_excerpt_down(cx: &mut Context) {
    expand(cx, MultiBufferExpand::Down);
}

pub(super) fn goto_next_multibuffer_excerpt(cx: &mut Context) {
    let view = view!(cx.editor);
    if let Err(err) = cx
        .editor
        .goto_next_multibuffer_excerpt(view.document_id(), view.id)
    {
        cx.editor.set_error(|| err.to_string());
    }
}
pub(super) fn goto_previous_multibuffer_excerpt_end(cx: &mut Context) {
    let view = view!(cx.editor);
    if let Err(err) = cx
        .editor
        .goto_previous_multibuffer_excerpt_end(view.document_id(), view.id)
    {
        cx.editor.set_error(|| err.to_string());
    }
}
pub(super) fn open_multibuffer_excerpt_vsplit(cx: &mut Context) {
    let view = view!(cx.editor);
    if let Err(err) =
        cx.editor
            .open_multibuffer_excerpt(view.document_id(), view.id, Action::VerticalSplit)
    {
        cx.editor.set_error(|| err.to_string());
    }
}

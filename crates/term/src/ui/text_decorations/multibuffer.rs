//! Source titles for shared excerpt layout annotations.

use theme::Theme;
use tui::buffer::Buffer;
use view::{
    graphics::{Rect, RectExt},
    Document, View,
};

use super::DecorationManager;
use crate::ui::document::{LinePos, TextRenderer};

pub fn add_headers<'a>(
    doc: &'a Document,
    view: &View,
    theme: &Theme,
    surface: &mut Buffer,
    decorations: &mut DecorationManager<'a>,
) {
    let Some(multi) = doc.multibuffer() else {
        return;
    };
    let area = view.content_area(doc);
    let inner = view.inner_area(doc);
    let style = theme.try_get("ui.multibuffer.header").unwrap_or_else(|| {
        theme
            .get("ui.statusline.inactive")
            .patch(theme.get("ui.virtual"))
    });
    // Keep the first visible source identifiable, including when its initial
    // inline header has scrolled out of the text viewport.
    let anchor = doc.view_offset(view.id).anchor;
    if !area.is_empty()
        && let Some(index) = doc.multibuffer_segment_index_at_char(anchor)
    {
        let header = area.with_height(1);
        surface.set_style(header, style);
        surface.set_stringn(
            inner.x,
            header.y,
            format!(
                " {} ",
                multi.source(multi.segments()[index].source).display_name
            ),
            inner.width as usize,
            style,
        );
    }
    decorations.add_decoration(move |renderer: &mut TextRenderer, pos: LinePos| {
        if pos.visual_line == 0 {
            return;
        }
        if let Some(segment) = multi
            .segments()
            .iter()
            .find(|s| s.projection_range.start == pos.doc_char)
        {
            let y = pos.visual_line - 1;
            renderer.set_style(Rect::new(area.x, y, area.width, 1), style);
            renderer.set_stringn(
                renderer.viewport.x,
                y,
                &format!(" {} ", multi.source(segment.source).display_name),
                renderer.viewport.width as usize,
                style,
            );
        }
    });
}

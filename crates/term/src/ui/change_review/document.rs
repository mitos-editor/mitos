//! Review-specific styling over the ordinary editor text renderer. No input or
//! viewport state lives here; document/view coordinates remain authoritative.

use stdx::rope::RopeExt as _;

use editor_core::highlight::OverlayHighlights;
use theme::Theme;
use view::{features::change_review::ReviewLineKind, graphics::Rect, Document};

use crate::ui::{
    document::{LinePos, TextRenderer},
    text_decorations::Decoration,
};

pub fn line_backgrounds<'a>(doc: &'a Document, theme: &'a Theme) -> impl Decoration + 'a {
    move |renderer: &mut TextRenderer, pos: LinePos| {
        let Some(row) = doc.review().and_then(|review| review.row(pos.doc_line)) else {
            return;
        };
        if matches!(row.kind, ReviewLineKind::Added | ReviewLineKind::Removed) {
            let mut style = super::line_style(theme, row.kind);
            style.fg = None;
            renderer.set_style(
                Rect::new(
                    renderer.viewport.x,
                    pos.visual_line,
                    renderer.viewport.width,
                    1,
                ),
                style,
            );
        }
    }
}

pub fn highlights(
    doc: &Document,
    anchor: usize,
    height: u16,
    theme: &Theme,
) -> Option<OverlayHighlights> {
    let review = doc.review()?;
    let text = doc.text();
    let first = text.char_to_line_idx(anchor.min(text.len_chars()));
    let last = (first + height as usize + 1).min(text.len_lines(stdx::rope::LINE_TYPE));
    let mut highlights = Vec::new();
    for index in first..last {
        let Some(row) = review.row(index) else {
            continue;
        };
        let start = text.line_to_char_idx(index);
        let scope = match row.kind {
            ReviewLineKind::Added => "diff.plus",
            ReviewLineKind::Removed => "diff.minus",
            ReviewLineKind::Header => "diff.delta",
            ReviewLineKind::Notice => "ui.text.inactive",
            ReviewLineKind::Context => continue,
        };
        if let Some(ranges) = review.changes(index) {
            let highlight = theme.find_highlight(&format!("{scope}.word"))?;
            for range in ranges {
                highlights.push((highlight, start + range.start..start + range.end));
            }
        } else if let Some(highlight) = theme.find_highlight(scope) {
            highlights.push((
                highlight,
                start
                    ..start
                        + text
                            .line(index, stdx::rope::LINE_TYPE)
                            .len_chars()
                            .saturating_sub(1),
            ));
        }
    }
    Some(OverlayHighlights::Heterogenous { highlights })
}

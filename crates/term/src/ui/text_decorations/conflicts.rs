//! Conflict sections use their own colors, independently of ordinary Git diffs.
use super::Decoration;
use theme::{Color, Style, Theme};
use view::{features::conflicts::ConflictSide, graphics::Rect, Document};

pub fn side_style(theme: &Theme, side: ConflictSide) -> Style {
    theme
        .try_get(side.scope())
        .unwrap_or_else(|| theme.get(side.fallback()))
}

pub fn line_backgrounds<'a>(doc: &'a Document, theme: &'a Theme) -> impl Decoration + 'a {
    move |renderer: &mut super::TextRenderer, pos: crate::ui::document::LinePos| {
        let Some(side) = doc
            .conflict_presentation()
            .side_at_line(doc.text(), pos.doc_line)
        else {
            return;
        };
        let style = side_style(theme, side);
        let background = match (theme.get("ui.background").bg, style.fg) {
            (Some(Color::Rgb(r, g, b)), Some(Color::Rgb(fr, fg, fb))) => Some(Color::Rgb(
                ((r as u16 * 15 + fr as u16) / 16) as u8,
                ((g as u16 * 15 + fg as u16) / 16) as u8,
                ((b as u16 * 15 + fb as u16) / 16) as u8,
            )),
            _ => None,
        };
        renderer.set_style(
            Rect::new(
                renderer.viewport.x,
                pos.visual_line,
                renderer.viewport.width,
                1,
            ),
            Style {
                bg: background,
                ..Style::default()
            },
        );
    }
}

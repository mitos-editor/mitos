//! Conflict sections use side-specific theme overrides with semantic color fallbacks.
use super::Decoration;
use theme::{Style, Theme};
use view::document::PresentedDocument;
#[cfg(test)]
use view::Document;
use view::{features::conflicts::ConflictSide, graphics::Rect};

pub fn side_style(theme: &Theme, side: ConflictSide) -> Style {
    theme
        .try_get_exact(side.scope())
        .or_else(|| theme.try_get_exact("ui.virtual.conflict"))
        .unwrap_or_else(|| theme.get(side.fallback()))
}

pub fn line_backgrounds<'a>(doc: PresentedDocument<'a>, theme: &'a Theme) -> impl Decoration + 'a {
    move |renderer: &mut super::TextRenderer, pos: crate::ui::document::LinePos| {
        let Some(side) = doc
            .conflict_presentation()
            .side_at_line(doc.text(), pos.doc_line)
        else {
            return;
        };
        let style = side_style(theme, side);
        let background = style.bg.or_else(|| theme.tinted_background(style.fg));
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

#[cfg(test)]
mod tests {
    use super::*;
    use theme::Color;

    #[test]
    fn conflict_backgrounds_tint_text_and_the_rest_of_the_row() {
        use std::sync::Arc;
        let theme = Theme::from(
            toml::from_str::<toml::Value>(
                r##"
            "ui.background" = { bg = "#101010" }
            "ui.text" = "#ffffff"
            "ui.selection" = { bg = "#505050" }
            "ui.virtual" = { fg = "#777777", bg = "#111111" }
            "diff.plus" = "#109010"
            info = "#101090"
            "ui.virtual.conflict.base" = { fg = "#909010", bg = "#303010" }
        "##,
            )
            .unwrap(),
        );
        for (source, incoming_line, end_line, neutral_lines) in [
            (
                "<<<<<<< HEAD\n界\n=======\n界\n>>>>>>> topic\n",
                3,
                4,
                &[][..],
            ),
            (
                "<<<<<<< HEAD\n界\n||||||| base\nold\n=======\n界\n>>>>>>> topic\n",
                5,
                6,
                &[2, 4][..],
            ),
        ] {
            let mut doc = Document::from(
                editor_core::Rope::from(source),
                None,
                Arc::new(arc_swap::ArcSwap::from_pointee(
                    editor_config::Config::default(),
                )),
                Arc::new(language::syntax::Loader::default()),
            );
            let mut view = view::View::new(
                doc.id(),
                editor_config::GutterConfig {
                    layout: Vec::new(),
                    ..Default::default()
                },
                event::Redraw::default(),
            );
            view.area = Rect::new(2, 3, 40, 8);
            doc.ensure_view_init(view.id);
            let layout = view.layout(&doc, Some(&theme));
            let mut surface = tui::buffer::Buffer::empty(Rect::new(0, 0, 50, 15));
            surface.set_style(surface.area, theme.get("ui.background"));
            let mut decorations = super::super::DecorationManager::default();
            decorations.add_decoration(line_backgrounds((&doc).into(), &theme));
            crate::ui::document::render_document_with_styles(
                &mut surface,
                (&doc).into(),
                &layout,
                Vec::new(),
                &theme,
                decorations,
                std::iter::empty(),
            );
            let x = layout.viewport.x;
            let y = layout.viewport.y;
            assert_eq!(surface[(x, y + end_line)].fg, Color::Rgb(16, 16, 144));
            for col in [x, x + 1, x + layout.viewport.width - 1] {
                assert_eq!(surface[(col, y + 1)].bg, Color::Rgb(16, 32, 16));
                assert_eq!(surface[(col, y + incoming_line)].bg, Color::Rgb(16, 16, 32));
                assert_eq!(surface[(col, y + end_line)].bg, Color::Rgb(16, 16, 32));
            }
            for &line in neutral_lines {
                assert_eq!(surface[(x, y + line)].fg, Color::Rgb(255, 255, 255));
                for col in [x, x + 1, x + layout.viewport.width - 1] {
                    assert_eq!(surface[(col, y + line)].bg, Color::Rgb(16, 16, 16));
                }
            }
            if !neutral_lines.is_empty() {
                assert_eq!(surface[(x, y + 3)].bg, Color::Rgb(48, 48, 16));
            }
            // Selection overlays must override the tint across the whole wide glyph.
            let current = &doc.conflict_presentation().regions[0].current;
            let selected = editor_core::highlight::OverlayHighlights::single(
                theme.find_highlight_exact("ui.selection").unwrap(),
                doc.text().byte_to_char_idx(current.start)
                    ..doc.text().byte_to_char_idx(current.end),
            );
            let mut surface = tui::buffer::Buffer::empty(surface.area);
            surface.set_style(surface.area, theme.get("ui.background"));
            let mut decorations = super::super::DecorationManager::default();
            decorations.add_decoration(line_backgrounds((&doc).into(), &theme));
            crate::ui::document::render_document_with_styles(
                &mut surface,
                (&doc).into(),
                &layout,
                vec![selected],
                &theme,
                decorations,
                std::iter::empty(),
            );
            assert_eq!(surface[(x, y + 1)].bg, Color::Rgb(80, 80, 80));
            assert_eq!(surface[(x + 1, y + 1)].bg, Color::Rgb(80, 80, 80));
            assert_eq!(
                surface[(x + layout.viewport.width - 1, y + 1)].bg,
                Color::Rgb(16, 32, 16)
            );
        }
    }

    #[test]
    fn conflict_colors_follow_theme_tokens_and_allow_side_overrides() {
        let theme = Theme::from(
            toml::from_str::<toml::Value>(
                r##"
            "ui.virtual" = { fg = "#777777", bg = "#111111" }
            "diff.plus" = "#00aa00"
            "info" = "#0000aa"
            "ui.virtual.conflict.base" = "#aaaa00"
        "##,
            )
            .unwrap(),
        );
        assert_eq!(
            side_style(&theme, ConflictSide::Current).fg,
            Some(Color::Rgb(0, 170, 0))
        );
        assert_eq!(
            side_style(&theme, ConflictSide::Incoming).fg,
            Some(Color::Rgb(0, 0, 170))
        );
        assert_eq!(
            side_style(&theme, ConflictSide::Base).fg,
            Some(Color::Rgb(170, 170, 0))
        );
        let theme = Theme::from(
            toml::from_str::<toml::Value>(
                r##"
            "ui.virtual" = { fg = "#777777", bg = "#111111" }
            "diff.plus" = "#00aa00"
            "ui.virtual.conflict.current" = { fg = "#aabbcc", bg = "#112233" }
        "##,
            )
            .unwrap(),
        );
        let style = side_style(&theme, ConflictSide::Current);
        assert_eq!(style.fg, Some(Color::Rgb(170, 187, 204)));
        assert_eq!(style.bg, Some(Color::Rgb(17, 34, 51)));
    }
}

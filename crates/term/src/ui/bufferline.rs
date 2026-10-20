//! Terminal buffer tabs, clipped in display cells rather than Unicode scalar values.
use editor_core::unicode::width::UnicodeWidthStr;
use tui::{buffer::Buffer, layout::Rect, style::Style, text::Line};

pub(super) fn render(titles: &[Line<'_>], active: usize, area: Rect, surface: &mut Buffer) {
    if area.is_empty() {
        return;
    }
    let width = usize::from(area.width);
    let total: usize = titles.iter().map(Line::width).sum();
    let start: usize = titles.iter().take(active).map(Line::width).sum();
    let active_width = titles.get(active).map_or(0, Line::width);
    // Center the active tab when possible. A tab wider than the viewport starts at
    // its beginning so the filename remains readable. Never scroll beyond the end.
    let scroll = start
        .saturating_sub(width.saturating_sub(active_width) / 2)
        .min(total.saturating_sub(width));

    let mut position = 0;
    for title in titles {
        let end = position + title.width();
        if end <= scroll {
            position = end;
            continue;
        }
        for grapheme in title.styled_graphemes(Style::default()) {
            let end = position + grapheme.symbol.width();
            if end > scroll && position < scroll + width {
                let visible_start = position.max(scroll);
                let visible_end = end.min(scroll + width);
                let x = area.x + (visible_start - scroll) as u16;
                if position >= scroll && end <= scroll + width {
                    surface.set_string(x, area.y, grapheme.symbol, grapheme.style);
                } else {
                    // A wide glyph crossing either edge occupies blank cells; never
                    // shift the following grapheme or paint outside the viewport.
                    for x in x..x + (visible_end - visible_start) as u16 {
                        surface[(x, area.y)]
                            .set_symbol(" ")
                            .set_style(grapheme.style);
                    }
                }
            }
            position = end;
            if position >= scroll + width {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::{
        style::{Color, Style},
        text::Span,
    };

    fn row(surface: &Buffer, area: Rect) -> String {
        let mut text = String::new();
        let mut x = area.x;
        while x < area.right() {
            let symbol = surface[(x, area.y)].symbol();
            text.push_str(symbol);
            x += Line::from(symbol).width().max(1) as u16;
        }
        text
    }

    #[test]
    fn keeps_every_active_tab_visible_and_returns_to_start() {
        let titles: Vec<_> = [" one ", " two ", " three ", " four "]
            .into_iter()
            .map(Line::from)
            .collect();
        let area = Rect::new(3, 2, 12, 1);
        for (active, label) in titles.iter().enumerate() {
            let mut surface = Buffer::empty(Rect::new(0, 0, 18, 4));
            render(&titles, active, area, &mut surface);
            assert!(row(&surface, area).contains(&label.to_string()));
            assert_eq!(surface[(2, 2)].symbol(), " ");
            assert_eq!(surface[(15, 2)].symbol(), " ");
        }
        let mut surface = Buffer::empty(area);
        render(&titles, 0, area, &mut surface);
        assert!(row(&surface, area).starts_with(" one "));
    }

    #[test]
    fn preserves_unicode_icons_combining_marks_and_styles() {
        let titles = [
            Line::from(" before "),
            Line::from(vec![
                Span::styled("🦀", Style::default().fg(Color::Red)),
                Span::raw(" cafe\u{301}界 "),
            ]),
            Line::from(" after "),
        ];
        let area = Rect::new(0, 0, 12, 1);
        let mut surface = Buffer::empty(area);
        render(&titles, 1, area, &mut surface);
        assert!(row(&surface, area).contains("🦀 cafe\u{301}界 "));
        let icon = surface
            .content
            .iter()
            .find(|cell| cell.symbol() == "🦀")
            .unwrap();
        assert_eq!(icon.fg, Color::Red);
    }

    #[test]
    fn long_tab_starts_at_filename_and_small_areas_are_safe() {
        let titles = [
            Line::from(" before "),
            Line::from(" extremely-long-filename "),
            Line::from(" end "),
        ];
        let area = Rect::new(0, 0, 8, 1);
        let mut surface = Buffer::empty(area);
        render(&titles, 1, area, &mut surface);
        assert_eq!(row(&surface, area), " extreme");
        for width in 0..3 {
            let area = Rect::new(0, 0, width, 1);
            render(&titles, 1, area, &mut Buffer::empty(area));
        }
        render(&[], 0, area, &mut surface);
    }

    #[test]
    fn wide_graphemes_crossing_either_edge_leave_blank_cells() {
        for wide in ["界", "🦀"] {
            let titles = [Line::from(format!("{wide}a")), Line::from(" b ")];
            let area = Rect::new(0, 0, 5, 1);
            let mut surface = Buffer::empty(area);
            render(&titles, 1, area, &mut surface);
            assert_eq!(row(&surface, area), " a b ");

            let area = Rect::new(0, 0, 1, 1);
            let mut surface = Buffer::empty(area);
            render(&titles, 0, area, &mut surface);
            assert_eq!(row(&surface, area), " ");
        }
    }

    #[test]
    fn totals_larger_than_u16_do_not_overflow() {
        let titles = vec![Line::from(" buffer "); 10_000];
        let area = Rect::new(0, 0, 16, 1);
        let mut surface = Buffer::empty(area);
        render(&titles, titles.len() - 1, area, &mut surface);
        assert_eq!(row(&surface, area), " buffer  buffer ");
    }
}

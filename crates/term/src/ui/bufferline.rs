//! Scroll at tab boundaries and delegate styled text clipping to Ratatui.
use super::text::visible_grapheme_widths;
use tui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};
use view::graphics::RectExt as _;

pub(super) fn render(
    titles: &[Line<'_>],
    active: usize,
    separator: Span<'_>,
    mut area: Rect,
    surface: &mut Buffer,
) {
    if area.is_empty() {
        return;
    }
    let Some(active_title) = titles.get(active) else {
        return;
    };
    let separator_width = visible_grapheme_widths(&separator.content).sum::<usize>();
    // Include preceding tabs while the active tab still fits. An oversized active
    // tab starts at the left edge so its filename is visible even in narrow views.
    let mut first = active;
    let mut width = active_title.width();
    while first > 0 {
        let with_previous = width
            .saturating_add(titles[first - 1].width())
            .saturating_add(separator_width);
        if with_previous > usize::from(area.width) {
            break;
        }
        width = with_previous;
        first -= 1;
    }

    for (index, title) in titles[first..].iter().enumerate() {
        if area.is_empty() {
            break;
        }
        if index > 0 && separator_width > 0 {
            let first_width = title
                .spans
                .iter()
                .flat_map(|span| visible_grapheme_widths(&span.content))
                .next();
            if first_width
                .is_none_or(|width| separator_width.saturating_add(width) > usize::from(area.width))
            {
                break;
            }
            surface.set_stringn(
                area.x,
                area.y,
                &separator.content,
                separator_width,
                separator.style,
            );
            area = area.clip_left(separator_width as u16);
        }
        let width = title.width().min(usize::from(area.width)) as u16;
        title.render(Rect { width, ..area }, surface);
        area = area.clip_left(width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::style::{Color, Style};

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
            render(&titles, active, Span::default(), area, &mut surface);
            assert!(row(&surface, area).contains(&label.to_string()));
            assert_eq!(surface[(2, 2)].symbol(), " ");
            assert_eq!(surface[(15, 2)].symbol(), " ");
        }
        let mut surface = Buffer::empty(area);
        render(&titles, 0, Span::default(), area, &mut surface);
        assert!(row(&surface, area).starts_with(" one "));
    }

    #[test]
    fn wide_separator_counts_toward_scrolling_and_only_joins_visible_tabs() {
        let titles = ["aa", "bb", "語x"].map(Line::from);
        let style = Style::default().fg(Color::Red).bg(Color::Blue);
        for (width, expected) in [(6, "bb    "), (7, "aa界·bb"), (12, "aa界·bb界·語")] {
            let area = Rect::new(2, 1, width, 1);
            let mut surface = Buffer::empty(area);
            render(&titles, 1, Span::styled("界·", style), area, &mut surface);
            assert_eq!(row(&surface, area), expected);

            if width >= 7 {
                assert_eq!(surface[(area.x + 2, area.y)].fg, Color::Red);
                assert_eq!(surface[(area.x + 2, area.y)].bg, Color::Blue);
            }
        }
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
        render(&titles, 1, Span::default(), area, &mut surface);
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
        render(&titles, 1, Span::default(), area, &mut surface);
        assert_eq!(row(&surface, area), " extreme");
        for width in 0..3 {
            let area = Rect::new(0, 0, width, 1);
            render(&titles, 1, Span::default(), area, &mut Buffer::empty(area));
        }
        render(&[], 0, Span::default(), area, &mut surface);
    }

    #[test]
    fn scrolling_starts_at_a_tab_boundary_and_clips_wide_glyphs_on_the_right() {
        for wide in ["界", "🦀"] {
            let titles = [Line::from(format!("{wide}a")), Line::from(" b ")];
            let area = Rect::new(0, 0, 5, 1);
            let mut surface = Buffer::empty(area);
            render(&titles, 1, Span::default(), area, &mut surface);
            assert_eq!(row(&surface, area), " b   ");

            let area = Rect::new(0, 0, 1, 1);
            let mut surface = Buffer::empty(area);
            render(&titles, 0, Span::default(), area, &mut surface);
            assert_eq!(row(&surface, area), " ");
        }
    }

    #[test]
    fn clipping_a_wide_icon_does_not_pull_later_spans_into_view() {
        let titles = [Line::from(vec![
            Span::styled("🦀", Style::default().fg(Color::Red)),
            Span::raw(" filename "),
        ])];
        let area = Rect::new(0, 0, 1, 1);
        let mut surface = Buffer::empty(area);
        render(&titles, 0, Span::default(), area, &mut surface);
        assert_eq!(row(&surface, area), " ");

        let area = Rect::new(0, 0, 3, 1);
        let mut surface = Buffer::empty(area);
        render(&titles, 0, Span::default(), area, &mut surface);
        assert_eq!(row(&surface, area), "🦀 ");
        assert_eq!(surface[(0, 0)].fg, Color::Red);
    }

    #[test]
    fn resizing_and_closing_buffers_restore_preceding_tabs() {
        let titles: Vec<_> = [" first ", " active ", " last "]
            .into_iter()
            .map(Line::from)
            .collect();
        let area = Rect::new(0, 0, 8, 1);
        let mut surface = Buffer::empty(area);
        render(&titles, 1, Span::default(), area, &mut surface);
        assert_eq!(row(&surface, area), " active ");

        let area = Rect::new(0, 0, 24, 1);
        let mut surface = Buffer::empty(area);
        render(&titles, 1, Span::default(), area, &mut surface);
        assert_eq!(row(&surface, area), " first  active  last    ");

        let area = Rect::new(0, 0, 8, 1);
        let mut surface = Buffer::empty(area);
        render(&titles[1..], 0, Span::default(), area, &mut surface);
        assert_eq!(row(&surface, area), " active ");
    }

    #[test]
    fn totals_larger_than_u16_do_not_overflow() {
        let titles = vec![Line::from(" buffer "); 10_000];
        let area = Rect::new(0, 0, 16, 1);
        let mut surface = Buffer::empty(area);
        render(
            &titles,
            titles.len() - 1,
            Span::default(),
            area,
            &mut surface,
        );
        assert_eq!(row(&surface, area), " buffer  buffer ");

        // Skipping a single huge title must not truncate its width to u16.
        let titles = [Line::from("x".repeat(70_000)), Line::from(" active ")];
        let mut surface = Buffer::empty(area);
        render(&titles, 1, Span::default(), area, &mut surface);
        assert_eq!(row(&surface, area), " active         ");
    }
}

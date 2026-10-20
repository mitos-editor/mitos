//! Scroll at tab boundaries and delegate styled text clipping to Ratatui.
use tui::{buffer::Buffer, layout::Rect, text::Line, widgets::Widget};

pub(super) fn render(titles: &[Line<'_>], active: usize, mut area: Rect, surface: &mut Buffer) {
    if area.is_empty() {
        return;
    }
    let Some(active_title) = titles.get(active) else {
        return;
    };
    // Include preceding tabs while the active tab still fits. An oversized active
    // tab starts at the left edge so its filename is visible even in narrow views.
    let mut first = active;
    let mut width = active_title.width();
    while first > 0 {
        let previous = titles[first - 1].width();
        if width.saturating_add(previous) > usize::from(area.width) {
            break;
        }
        width += previous;
        first -= 1;
    }

    for title in &titles[first..] {
        if area.is_empty() {
            break;
        }
        let width = title.width().min(usize::from(area.width)) as u16;
        title.render(Rect { width, ..area }, surface);
        area.x += width;
        area.width -= width;
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
    fn scrolling_starts_at_a_tab_boundary_and_clips_wide_glyphs_on_the_right() {
        for wide in ["界", "🦀"] {
            let titles = [Line::from(format!("{wide}a")), Line::from(" b ")];
            let area = Rect::new(0, 0, 5, 1);
            let mut surface = Buffer::empty(area);
            render(&titles, 1, area, &mut surface);
            assert_eq!(row(&surface, area), " b   ");

            let area = Rect::new(0, 0, 1, 1);
            let mut surface = Buffer::empty(area);
            render(&titles, 0, area, &mut surface);
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
        render(&titles, 0, area, &mut surface);
        assert_eq!(row(&surface, area), " ");

        let area = Rect::new(0, 0, 3, 1);
        let mut surface = Buffer::empty(area);
        render(&titles, 0, area, &mut surface);
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
        render(&titles, 1, area, &mut surface);
        assert_eq!(row(&surface, area), " active ");

        let area = Rect::new(0, 0, 24, 1);
        let mut surface = Buffer::empty(area);
        render(&titles, 1, area, &mut surface);
        assert_eq!(row(&surface, area), " first  active  last    ");

        let area = Rect::new(0, 0, 8, 1);
        let mut surface = Buffer::empty(area);
        render(&titles[1..], 0, area, &mut surface);
        assert_eq!(row(&surface, area), " active ");
    }

    #[test]
    fn totals_larger_than_u16_do_not_overflow() {
        let titles = vec![Line::from(" buffer "); 10_000];
        let area = Rect::new(0, 0, 16, 1);
        let mut surface = Buffer::empty(area);
        render(&titles, titles.len() - 1, area, &mut surface);
        assert_eq!(row(&surface, area), " buffer  buffer ");

        // Skipping a single huge title must not truncate its width to u16.
        let titles = [Line::from("x".repeat(70_000)), Line::from(" active ")];
        let mut surface = Buffer::empty(area);
        render(&titles, 1, area, &mut surface);
        assert_eq!(row(&surface, area), " active         ");
    }
}

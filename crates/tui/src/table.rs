//! Row preparation for suffix-preserving table truncation.
//! Table construction, styling, selection, and rendering belong to Ratatui.

use ratatui::{
    layout::{Constraint, Flex, Layout, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{Cell as RatatuiCell, Row as RatatuiRow},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
/// Styled text stored in one table cell.
pub struct Cell<'a> {
    /// Cell contents; line height is determined by Ratatui during rendering.
    pub content: Text<'a>,
    prefix: Option<Span<'a>>,
}

impl<'a> Cell<'a> {
    /// Adds a decorative prefix that stays visible when the text is truncated.
    pub fn with_prefix(mut self, prefix: Span<'a>) -> Self {
        self.prefix = Some(prefix);
        self
    }
}

impl<'a, T> From<T> for Cell<'a>
where
    T: Into<Text<'a>>,
{
    fn from(content: T) -> Self {
        Self {
            content: content.into(),
            prefix: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
/// A row of cells with a shared base style.
pub struct Row<'a> {
    /// Cells rendered from left to right.
    pub cells: Vec<Cell<'a>>,
    style: Style,
}

impl<'a> Row<'a> {
    /// Builds a row from values convertible to [`Cell`].
    pub fn new<T>(cells: T) -> Self
    where
        T: IntoIterator,
        T::Item: Into<Cell<'a>>,
    {
        Self {
            cells: cells.into_iter().map(Into::into).collect(),
            ..Default::default()
        }
    }

    /// Applies a base style to every cell in this row.
    pub fn style<S: Into<Style>>(mut self, style: S) -> Self {
        self.style = style.into();
        self
    }
}

impl<'a, T: Into<Cell<'a>>> From<T> for Row<'a> {
    fn from(cell: T) -> Self {
        Self::new([cell.into()])
    }
}

/// Truncate rows to their allocated columns before passing them to Ratatui.
/// Decorative prefixes retain their width, text, and style on the first line.
/// Use the same spacing, selection-marker width, and `Flex::Legacy` layout on the table.
pub fn truncate_rows_start(
    rows: &mut [Row<'_>],
    area: Rect,
    widths: &[Constraint],
    column_spacing: u16,
    selection_marker: &str,
) {
    let constraints = if widths.is_empty() {
        let count = rows
            .iter()
            .map(|row| row.cells.len())
            .max()
            .unwrap_or_default();
        vec![Constraint::Length(area.width / count.max(1) as u16); count]
    } else {
        widths.to_vec()
    };
    let [_, columns_area] = Layout::horizontal([
        Constraint::Length(selection_marker.width() as u16),
        Constraint::Fill(0),
    ])
    .areas(area);
    let columns = Layout::horizontal(constraints)
        .flex(Flex::Legacy)
        .spacing(column_spacing)
        .split(columns_area);
    for row in rows {
        for (cell, column) in row.cells.iter_mut().zip(columns.iter()) {
            for (index, line) in cell.content.lines.iter_mut().enumerate() {
                let prefix_width = if index == 0 {
                    cell.prefix.as_ref().map_or(0, Span::width)
                } else {
                    0
                };
                truncate_line_start(line, (column.width as usize).saturating_sub(prefix_width));
            }
        }
    }
}

impl<'a> From<Row<'a>> for RatatuiRow<'a> {
    fn from(row: Row<'a>) -> Self {
        let cells = row.cells.into_iter().map(|mut cell| {
            if let Some(prefix) = cell.prefix
                && let Some(line) = cell.content.lines.first_mut()
            {
                line.spans.insert(0, prefix);
            }
            RatatuiCell::new(cell.content)
        });
        RatatuiRow::new(cells).style(row.style)
    }
}

fn truncate_line_start(line: &mut Line<'_>, width: usize) {
    if line.width() <= width {
        return;
    }
    if width == 0 {
        line.spans.clear();
        return;
    }

    let mut remaining = width.saturating_sub(1);
    let mut spans = Vec::new();
    let mut exhausted = false;
    for span in line.spans.iter().rev() {
        let mut content = String::new();
        for grapheme in span.content.graphemes(true).rev() {
            let grapheme_width = grapheme.width();
            if grapheme_width > remaining {
                exhausted = true;
                break;
            }
            content.insert_str(0, grapheme);
            remaining -= grapheme_width;
        }
        if !content.is_empty() {
            spans.push(Span::styled(content, span.style));
        }
        if remaining == 0 || exhausted {
            break;
        }
    }
    spans.reverse();
    spans.insert(0, Span::raw("…"));
    line.spans = spans;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        buffer::Buffer,
        widgets::{StatefulWidget, Table, TableState},
    };

    fn render(
        rows: impl IntoIterator<Item = Row<'static>>,
        widths: &[Constraint],
        area: Rect,
        selected: bool,
    ) -> Buffer {
        let mut rows: Vec<_> = rows.into_iter().collect();
        let marker = if selected { " > " } else { "" };
        truncate_rows_start(&mut rows, area, widths, 1, marker);
        let mut buffer = Buffer::empty(area);
        let mut state = TableState::default().with_selected(selected.then_some(0));
        StatefulWidget::render(
            Table::new(rows, widths)
                .highlight_symbol(marker)
                .flex(Flex::Legacy),
            area,
            &mut buffer,
            &mut state,
        );
        buffer
    }

    #[test]
    fn renders_with_ratatui_and_preserves_start_truncation() {
        let buffer = render(
            [Row::new(["abcdef"])],
            &[Constraint::Length(5)],
            Rect::new(0, 0, 5, 1),
            false,
        );
        assert_eq!(buffer, Buffer::with_lines(["…cdef"]));
    }

    #[test]
    fn last_column_fills_remaining_width() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 5, 1));
        let table = Table::new(
            [Row::new(["a", "bcde"])],
            [Constraint::Length(1), Constraint::Length(1)],
        )
        .column_spacing(0)
        .flex(Flex::Legacy);
        StatefulWidget::render(table, buffer.area, &mut buffer, &mut TableState::default());
        assert_eq!(buffer, Buffer::with_lines(["abcde"]));
    }

    #[test]
    fn start_truncation_uses_available_width_and_keeps_short_rows_left_aligned() {
        let buffer = render(
            [Row::new(["long/path/to/file.rs"]), Row::new(["main.rs"])],
            &[Constraint::Length(20)],
            Rect::new(0, 0, 13, 2),
            true,
        );
        assert_eq!(
            buffer,
            Buffer::with_lines([" > …o/file.rs", "   main.rs   "])
        );
    }

    #[test]
    fn start_truncation_uses_allocated_width_for_each_column() {
        let buffer = render(
            [Row::new(["abcdefghij", "0123456789"])],
            &[Constraint::Percentage(50), Constraint::Percentage(50)],
            Rect::new(0, 0, 13, 1),
            false,
        );
        assert_eq!(buffer, Buffer::with_lines(["…efghij …6789"]));
    }

    #[test]
    fn start_truncation_preserves_prefix_and_grapheme_styles() {
        use ratatui::style::{Color, Modifier};
        let highlight = Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD);
        let icon_style = Style::default().fg(Color::Red);
        let text = Line::from(vec![
            Span::raw("long/path/"),
            Span::styled("界e\u{301}", highlight),
            Span::raw(".rs"),
        ]);
        let cell = Cell::from(text).with_prefix(Span::styled("🦀", icon_style));
        let buffer = render(
            [Row::new([cell])],
            &[Constraint::Length(30)],
            Rect::new(0, 0, 10, 1),
            true,
        );
        let expected = Line::from(vec![
            Span::raw(" > "),
            Span::styled("🦀", icon_style),
            Span::raw("…"),
            Span::styled("e\u{301}", highlight),
            Span::raw(".rs"),
        ]);
        assert_eq!(buffer, Buffer::with_lines([expected]));
    }

    #[test]
    fn start_truncation_does_not_skip_wide_graphemes_between_spans() {
        let text = Line::from(vec![Span::raw("ab"), Span::raw("界"), Span::raw("rs")]);
        let buffer = render(
            [Row::new([Cell::from(text)])],
            &[Constraint::Length(6)],
            Rect::new(0, 0, 4, 1),
            false,
        );
        assert_eq!(buffer, Buffer::with_lines(["…rs "]));
    }

    #[test]
    fn truncation_handles_zero_width_columns() {
        let buffer = render(
            [Row::new([
                Cell::from("long/path/file.rs").with_prefix(Span::raw("🦀"))
            ])],
            &[Constraint::Length(20)],
            Rect::new(0, 0, 0, 1),
            true,
        );
        assert!(buffer.content.is_empty());
    }
}

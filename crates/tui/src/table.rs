//! Table wrapper for editor-specific truncation behavior.
//!
//! Ratatui's table is still used for layout and rendering. This wrapper keeps
//! the historical Mitos API and can truncate overflowing cells from the start,
//! preserving filenames and other useful suffixes.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Flex, Layout, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{
        Cell as RatatuiCell, Row as RatatuiRow, StatefulWidget, Table as RatatuiTable, TableState,
    },
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

#[derive(Debug, Clone)]
/// Builder for a stateful Ratatui table with optional start truncation.
pub struct Table<'a> {
    rows: Vec<Row<'a>>,
    widths: Vec<Constraint>,
    style: Style,
    highlight_style: Style,
    highlight_symbol: Option<&'a str>,
    column_spacing: u16,
    header: Option<Row<'a>>,
}

impl<'a> Table<'a> {
    /// Builds a table from its body rows.
    pub fn new<T>(rows: T) -> Self
    where
        T: IntoIterator<Item = Row<'a>>,
    {
        Self {
            rows: rows.into_iter().collect(),
            widths: Vec::new(),
            style: Style::default(),
            highlight_style: Style::default(),
            highlight_symbol: None,
            column_spacing: 1,
            header: None,
        }
    }

    /// Sets the Ratatui constraints used to lay out columns.
    pub fn widths(mut self, widths: &[Constraint]) -> Self {
        self.widths = widths.to_vec();
        self
    }

    /// Applies the table's base style.
    pub fn style<S: Into<Style>>(mut self, style: S) -> Self {
        self.style = style.into();
        self
    }

    /// Sets the style for the row selected in [`TableState`].
    pub fn highlight_style<S: Into<Style>>(mut self, style: S) -> Self {
        self.highlight_style = style.into();
        self
    }

    /// Sets the marker rendered beside the selected row.
    pub fn highlight_symbol(mut self, symbol: &'a str) -> Self {
        self.highlight_symbol = Some(symbol);
        self
    }

    /// Sets the number of cells between columns.
    pub fn column_spacing(mut self, spacing: u16) -> Self {
        self.column_spacing = spacing;
        self
    }

    /// Adds a non-selectable header row.
    pub fn header(mut self, header: Row<'a>) -> Self {
        self.header = Some(header);
        self
    }

    /// Renders the table and updates `state` through Ratatui.
    ///
    /// When `truncate_start` is true, overflowing cells keep their suffix and
    /// receive a leading ellipsis within their allocated column width.
    pub fn render_table(
        self,
        area: Rect,
        buffer: &mut Buffer,
        state: &mut TableState,
        truncate_start: bool,
    ) {
        // Match Ratatui's layout, including the selection marker and spacing,
        // before truncating. Requested widths may exceed the available area.
        let selection_width = if state.selected().is_some() {
            self.highlight_symbol
                .map_or(0, |symbol| symbol.width() as u16)
        } else {
            0
        };
        let column_count = self
            .rows
            .iter()
            .chain(self.header.iter())
            .map(|row| row.cells.len())
            .max()
            .unwrap_or_default();
        let constraints = if self.widths.is_empty() {
            vec![Constraint::Length(area.width / column_count.max(1) as u16); column_count]
        } else {
            self.widths.clone()
        };
        let [_, columns_area] =
            Layout::horizontal([Constraint::Length(selection_width), Constraint::Fill(0)])
                .areas(area);
        let widths = Layout::horizontal(constraints)
            .flex(Flex::Legacy)
            .spacing(self.column_spacing)
            .split(columns_area);
        let rows = self
            .rows
            .into_iter()
            .map(|row| row.into_ratatui(&widths, truncate_start));
        let mut table = RatatuiTable::new(rows, self.widths)
            .style(self.style)
            .row_highlight_style(self.highlight_style)
            .column_spacing(self.column_spacing)
            .flex(Flex::Legacy);
        if let Some(symbol) = self.highlight_symbol {
            table = table.highlight_symbol(symbol);
        }
        if let Some(header) = self.header {
            table = table.header(header.into_ratatui(&widths, false));
        }

        StatefulWidget::render(table, area, buffer, state);
    }
}

impl<'a> Row<'a> {
    fn into_ratatui(self, widths: &[Rect], truncate_start: bool) -> RatatuiRow<'a> {
        let cells = self.cells.into_iter().enumerate().map(|(index, mut cell)| {
            if truncate_start && let Some(area) = widths.get(index) {
                for (line_index, line) in cell.content.lines.iter_mut().enumerate() {
                    let prefix_width = if line_index == 0 {
                        cell.prefix.as_ref().map_or(0, Span::width)
                    } else {
                        0
                    };
                    truncate_line_start(line, (area.width as usize).saturating_sub(prefix_width));
                }
            }
            if let Some(prefix) = cell.prefix
                && let Some(line) = cell.content.lines.first_mut()
            {
                line.spans.insert(0, prefix);
            }
            RatatuiCell::new(cell.content)
        });
        RatatuiRow::new(cells).style(self.style)
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

    #[test]
    fn renders_with_ratatui_and_preserves_start_truncation() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 5, 1));
        let mut state = TableState::default();
        let table = Table::new([Row::new(["abcdef"])]).widths(&[Constraint::Length(5)]);

        table.render_table(buffer.area, &mut buffer, &mut state, true);

        let rendered: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert_eq!(rendered, "…cdef");
    }

    #[test]
    fn last_column_fills_remaining_width() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 5, 1));
        let mut state = TableState::default();
        let table = Table::new([Row::new(["a", "bcde"])])
            .widths(&[Constraint::Length(1), Constraint::Length(1)])
            .column_spacing(0);

        table.render_table(buffer.area, &mut buffer, &mut state, false);

        let rendered: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert_eq!(rendered, "abcde");
    }

    #[test]
    fn start_truncation_uses_available_width_and_keeps_short_rows_left_aligned() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 13, 2));
        let mut state = TableState::default().with_selected(Some(0));
        let table = Table::new([Row::new(["long/path/to/file.rs"]), Row::new(["main.rs"])])
            .widths(&[Constraint::Length(20)])
            .highlight_symbol(" > ");

        table.render_table(buffer.area, &mut buffer, &mut state, true);

        assert_eq!(
            buffer,
            Buffer::with_lines([" > …o/file.rs", "   main.rs   "])
        );
    }

    #[test]
    fn start_truncation_uses_allocated_width_for_each_column() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 13, 1));
        let mut state = TableState::default();
        let table = Table::new([Row::new(["abcdefghij", "0123456789"])])
            .widths(&[Constraint::Percentage(50), Constraint::Percentage(50)])
            .column_spacing(1);

        table.render_table(buffer.area, &mut buffer, &mut state, true);

        assert_eq!(buffer, Buffer::with_lines(["…efghij …6789"]));
    }

    #[test]
    fn start_truncation_preserves_prefix_and_grapheme_styles() {
        use ratatui::style::{Color, Modifier};

        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 1));
        let mut state = TableState::default().with_selected(Some(0));
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
        let table = Table::new([Row::new([cell])])
            .widths(&[Constraint::Length(30)])
            .highlight_symbol(" > ");

        table.render_table(buffer.area, &mut buffer, &mut state, true);

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
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        let mut state = TableState::default();
        let text = Line::from(vec![Span::raw("ab"), Span::raw("界"), Span::raw("rs")]);
        let table = Table::new([Row::new([Cell::from(text)])]).widths(&[Constraint::Length(6)]);

        table.render_table(buffer.area, &mut buffer, &mut state, true);

        assert_eq!(buffer, Buffer::with_lines(["…rs "]));
    }

    #[test]
    fn truncation_handles_zero_width_columns() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 0, 1));
        let mut state = TableState::default().with_selected(Some(0));
        let table = Table::new([Row::new([
            Cell::from("long/path/file.rs").with_prefix(Span::raw("🦀"))
        ])])
        .widths(&[Constraint::Length(20)])
        .highlight_symbol(" > ");

        table.render_table(buffer.area, &mut buffer, &mut state, true);

        assert!(buffer.content.is_empty());
    }
}

//! Ratatui styled text and display-width truncation.

pub use ratatui::text::*;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Truncate a styled line to `width` columns, adding an ellipsis on the truncated side.
/// Set `from_start` to preserve the suffix instead of the prefix. Grapheme clusters
/// and span styles are preserved; a grapheme that does not fit stops traversal.
pub fn truncate_line(line: &mut Line<'_>, width: usize, from_start: bool) {
    if line.width() <= width {
        return;
    }
    if width == 0 {
        line.spans.clear();
        return;
    }

    let mut remaining = width - 1;
    let mut spans = Vec::new();
    let mut exhausted = false;
    for index in 0..line.spans.len() {
        let index = if from_start {
            line.spans.len() - 1 - index
        } else {
            index
        };
        let span = &line.spans[index];
        let mut graphemes = span.content.graphemes(true);
        let mut content = Vec::new();
        while let Some(grapheme) = if from_start {
            graphemes.next_back()
        } else {
            graphemes.next()
        } {
            let grapheme_width = grapheme.width();
            if grapheme_width > remaining {
                exhausted = true;
                break;
            }
            content.push(grapheme);
            remaining -= grapheme_width;
        }
        if from_start {
            content.reverse();
        }
        if !content.is_empty() {
            spans.push(Span::styled(content.concat(), span.style));
        }
        if exhausted {
            break;
        }
    }
    if from_start {
        spans.reverse();
        spans.insert(0, Span::raw("…"));
    } else {
        spans.push(Span::raw("…"));
    }
    line.spans = spans;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};

    #[test]
    fn truncation_preserves_graphemes_and_styles() {
        for (from_start, expected) in [(false, "界e\u{301}…"), (true, "…👩‍💻.rs")] {
            let style = Style::default().fg(Color::Red).bg(Color::Blue);
            let mut line = Line::from(Span::styled("界e\u{301}👩‍💻.rs", style));
            truncate_line(&mut line, if from_start { 6 } else { 4 }, from_start);
            assert_eq!(line.to_string(), expected);
            assert!(line
                .spans
                .iter()
                .filter(|span| span.content != "…")
                .all(|span| span.style == style));
        }
    }

    #[test]
    fn truncation_does_not_skip_wide_graphemes_between_spans() {
        for from_start in [false, true] {
            let mut line = Line::from(vec![Span::raw("ab"), Span::raw("界"), Span::raw("rs")]);
            truncate_line(&mut line, 4, from_start);
            assert_eq!(line.to_string(), if from_start { "…rs" } else { "ab…" });
        }
    }

    #[test]
    fn truncation_handles_zero_and_one_column_and_keeps_short_lines() {
        for from_start in [false, true] {
            for (width, expected) in [(0, ""), (1, "…"), (3, "abc"), (4, "abc")] {
                let mut line = Line::raw("abc");
                truncate_line(&mut line, width, from_start);
                assert_eq!(line.to_string(), expected);
            }
        }
    }
}

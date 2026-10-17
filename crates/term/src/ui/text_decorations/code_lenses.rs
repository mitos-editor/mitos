//! Clipped code lens labels after the final visual segment of a document line.

use std::ops::Range;

use editor_core::{
    line_ending::line_end_char_index,
    unicode::{segmentation::UnicodeSegmentation, width::UnicodeWidthStr},
    Position, Rope,
};
use text_layout::doc_formatter::FormattedGrapheme;
use theme::Style;
use view::document::CodeLens;

use super::Decoration;
use crate::ui::document::{LinePos, TextRenderer};

pub struct CodeLensDecoration<'a> {
    text: &'a Rope,
    lenses: &'a [CodeLens],
    style: Style,
    hidden_line: Option<usize>,
    index: usize,
    ready: Option<(usize, Range<usize>)>,
}

impl<'a> CodeLensDecoration<'a> {
    pub fn new(
        text: &'a Rope,
        lenses: &'a [CodeLens],
        style: Style,
        hidden_line: Option<usize>,
    ) -> Self {
        Self {
            text,
            lenses,
            style,
            hidden_line,
            index: 0,
            ready: None,
        }
    }

    fn next_anchor(&self) -> usize {
        self.lenses.get(self.index).map_or(usize::MAX, |lens| {
            line_end_char_index(&self.text.slice(..), self.text.char_to_line(lens.char_idx))
        })
    }
}

impl Decoration for CodeLensDecoration<'_> {
    fn reset_pos(&mut self, pos: usize) -> usize {
        self.ready = None;
        let line = self.text.char_to_line(pos.min(self.text.len_chars()));
        self.index = self
            .lenses
            .partition_point(|lens| self.text.char_to_line(lens.char_idx) < line);
        self.next_anchor()
    }

    fn decorate_grapheme(&mut self, _: &mut TextRenderer, grapheme: &FormattedGrapheme) -> usize {
        if grapheme.is_virtual() {
            return grapheme.char_idx;
        }
        let start = self.index;
        while self
            .lenses
            .get(self.index)
            .is_some_and(|lens| self.text.char_to_line(lens.char_idx) == grapheme.line_idx)
        {
            self.index += 1;
        }
        self.ready = Some((grapheme.line_idx, start..self.index));
        self.next_anchor()
    }

    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        off: Position,
    ) -> Position {
        let Some((line, indices)) = self.ready.take() else {
            return Position::new(0, 0);
        };
        if line != pos.doc_line
            || self.hidden_line == Some(line)
            || (pos.visual_line as usize) < renderer.offset.row
            || pos.visual_line as usize - renderer.offset.row >= renderer.viewport.height as usize
        {
            return Position::new(0, 0);
        }
        let titles: Vec<_> = self.lenses[indices]
            .iter()
            .filter_map(|lens| lens.lens.command.as_ref())
            .map(|command| {
                command
                    .title
                    .chars()
                    .map(|c| {
                        if c.is_control() || c == '\u{2028}' || c == '\u{2029}' {
                            ' '
                        } else {
                            c
                        }
                    })
                    .collect::<String>()
            })
            .filter(|title| !title.trim().is_empty())
            .collect();
        if titles.is_empty() {
            return Position::new(0, 0);
        }
        let labels = format!("▶ {}", titles.join(" · "));
        let start = off.col.saturating_add(1);
        let right = renderer.offset.col + renderer.viewport.width as usize;
        if start >= right {
            return Position::new(0, 0);
        }
        let col = start.saturating_sub(renderer.offset.col);
        let width = renderer.viewport.width as usize - col;
        // Leave the gutter/menu as the fallback if all that fits is the marker.
        if width < 4 {
            return Position::new(0, 0);
        }
        let mut logical_col = start;
        let visible: String = labels
            .graphemes(true)
            .filter_map(|grapheme| {
                let from = logical_col;
                logical_col += grapheme.width();
                if from >= renderer.offset.col {
                    Some(grapheme.to_owned())
                } else if logical_col > renderer.offset.col {
                    Some(" ".repeat(logical_col - renderer.offset.col))
                } else {
                    None
                }
            })
            .collect();
        if visible.is_empty() {
            return Position::new(0, 0);
        }
        let ellipsis = visible.width() > width;
        let (end, _) = renderer.set_string_truncated(
            renderer.viewport.x + col as u16,
            pos.visual_line,
            &visible,
            width,
            |_| self.style,
            ellipsis,
            false,
        );
        let logical_end = end.saturating_sub(renderer.viewport.x) as usize
            + renderer.offset.col
            + usize::from(ellipsis);
        Position::new(0, logical_end.saturating_sub(off.col))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{
        document::render_text,
        text_decorations::{DecorationManager, InlineDiagnostics},
    };
    use arc_swap::ArcSwap;
    use editor_core::{
        diagnostic::{DiagnosticProvider, Severity},
        Diagnostic,
    };
    use language::syntax;
    use lsp_client::{lsp, LanguageServerId};
    use std::sync::Arc;
    use theme::Theme;
    use tui::{buffer::Buffer, layout::Rect};
    use view::{
        annotations::diagnostics::{DiagnosticFilter, InlineDiagnosticsConfig},
        Document,
    };

    fn document(text: &str, soft_wrap: bool) -> Document {
        let mut config = view::config::Config::default();
        config.soft_wrap.enable = Some(soft_wrap);
        Document::from(
            Rope::from_str(text),
            None,
            Arc::new(ArcSwap::from_pointee(config)),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        )
    }

    fn lens(char_idx: usize, title: &str) -> CodeLens {
        CodeLens {
            char_idx,
            language_server_id: LanguageServerId::default(),
            lens: lsp::CodeLens {
                range: lsp::Range::default(),
                data: None,
                command: Some(lsp::Command {
                    title: title.into(),
                    command: "run".into(),
                    arguments: None,
                }),
            },
        }
    }

    fn render(
        doc: &Document,
        lenses: &[CodeLens],
        width: u16,
        offset: Position,
        hidden: Option<usize>,
    ) -> Buffer {
        let theme = Theme::default();
        let mut surface = Buffer::empty(Rect::new(0, 0, width + 4, 12));
        for cell in &mut surface.content {
            cell.set_symbol("#");
        }
        for y in 1..11 {
            for x in 2..width + 2 {
                surface[(x, y)].set_symbol(" ");
            }
        }
        let viewport = view::graphics::Rect::new(2, 1, width, 10);
        let mut renderer = TextRenderer::new(&mut surface, doc, &theme, offset, viewport);
        let mut decorations = DecorationManager::default();
        let config = InlineDiagnosticsConfig {
            cursor_line: DiagnosticFilter::Disable,
            other_lines: DiagnosticFilter::Disable,
            ..Default::default()
        };
        decorations.add_decoration(InlineDiagnostics::new(
            doc,
            &theme,
            0,
            config,
            DiagnosticFilter::Enable(Severity::Hint),
            false,
        ));
        decorations.add_decoration(CodeLensDecoration::new(
            doc.text(),
            lenses,
            Style::default(),
            hidden,
        ));
        render_text(
            &mut renderer,
            doc.text().slice(..),
            0,
            &doc.text_format(width, Some(&theme)),
            &text_layout::text_annotations::TextAnnotations::default(),
            None,
            vec![],
            &theme,
            decorations,
        );
        surface
    }

    fn row(surface: &Buffer, row: u16) -> String {
        (2..surface.area.width - 2)
            .map(|col| surface[(col, row + 1)].symbol())
            .collect()
    }

    #[test]
    fn labels_follow_source_at_eof_lf_and_crlf_without_adding_rows() {
        for text in ["abc", "abc\nnext", "abc\r\nnext"] {
            let doc = document(text, false);
            let lenses = [lens(0, "Run"), lens(1, "3 references")];
            let surface = render(&doc, &lenses, 40, Position::default(), None);
            assert!(
                row(&surface, 0).starts_with("abc  ▶ Run · 3 references"),
                "{}",
                row(&surface, 0)
            );
            if text.contains('\n') {
                assert!(row(&surface, 1).starts_with("next"));
            }
            assert!(!row(&surface, 1).contains("▶"));
        }
        let doc = document("abc", false);
        let exact = render(&doc, &[lens(0, "Run")], 10, Position::default(), None);
        assert_eq!(row(&exact, 0), "abc  ▶ Run");
    }

    #[test]
    fn softwrapped_labels_only_use_the_last_segment_and_do_not_reflow_source() {
        let doc = document("foo foo foo foo foo foo foo foo\nnext", true);
        let baseline = render(&doc, &[], 20, Position::default(), None);
        let surface = render(&doc, &[lens(0, "Run")], 20, Position::default(), None);
        let lens_rows: Vec<_> = (0..10)
            .filter(|row_idx| row(&surface, *row_idx).contains("▶"))
            .collect();
        assert_eq!(lens_rows.len(), 1);
        assert!(lens_rows[0] > 0);
        assert!(!row(&surface, 0).contains("▶"));
        assert_eq!(
            (0..10).find(|line| row(&baseline, *line).starts_with("next")),
            (0..10).find(|line| row(&surface, *line).starts_with("next"))
        );
        for y in 0..12 {
            for x in 0..surface.area.width {
                let symbol = baseline[(x, y)].symbol();
                if symbol != " " && symbol != "#" {
                    assert_eq!(surface[(x, y)].symbol(), symbol);
                }
            }
        }
    }

    #[test]
    fn labels_clip_unicode_and_control_characters_to_the_split() {
        let doc = document("\t界e\u{301}\r\nnext", false);
        let lenses = [lens(0, "界界界界界\nmore\ttext")];
        let surface = render(&doc, &lenses, 20, Position::default(), None);
        assert!(row(&surface, 0).contains("▶"));
        assert!(row(&surface, 0).contains('…'));
        assert!(row(&surface, 1).starts_with("next"));
        for y in 0..12 {
            for x in [0, 1, 22, 23] {
                assert_eq!(surface[(x, y)].symbol(), "#");
            }
        }
        let narrow = render(
            &document("abc", false),
            &[lens(0, "Run")],
            7,
            Position::default(),
            None,
        );
        assert!(!row(&narrow, 0).contains("▶"));
    }

    #[test]
    fn diagnostics_take_precedence_and_lenses_use_only_the_remaining_width() {
        let mut doc = document("abc", false);
        doc.replace_diagnostics(
            [Diagnostic {
                range: editor_core::Range::new(0, 1).into(),
                ends_at_word: false,
                starts_at_word: false,
                zero_width: false,
                line: 0,
                message: "error".into(),
                severity: Some(Severity::Error),
                code: None,
                provider: DiagnosticProvider::Spelling,
                tags: vec![],
                source: None,
                data: None,
            }],
            &[],
            None,
        );
        let lenses = [lens(0, "Run")];
        let surface = render(&doc, &lenses, 30, Position::default(), None);
        let line = row(&surface, 0);
        assert!(line.starts_with("abc  error"));
        assert!(line.find("error").unwrap() < line.find("▶ Run").unwrap());
        let narrow = render(&doc, &lenses, 12, Position::default(), None);
        assert!(row(&narrow, 0).contains("error"));
        assert!(!row(&narrow, 0).contains("▶"));
    }

    #[test]
    fn horizontal_scrolling_and_completion_suppression_preserve_the_viewport() {
        let doc = document("abcdef", false);
        let lenses = [lens(0, "界 Run")];
        let surface = render(&doc, &lenses, 15, Position::new(0, 10), None);
        assert!(
            row(&surface, 0).starts_with("界  Run"),
            "{}",
            row(&surface, 0)
        );
        let suppressed = render(&doc, &lenses, 30, Position::default(), Some(0));
        assert!(!row(&suppressed, 0).contains("▶"));
        let below = render(&doc, &lenses, 30, Position::new(11, 0), None);
        let baseline = render(&doc, &[], 30, Position::new(11, 0), None);
        assert_eq!(below.content, baseline.content);
    }
}

//! Runtime values share the formatter's wrapping, clipping, and decoration offsets.

use editor_core::{line_ending::line_end_char_index, Rope};
use text_layout::{doc_formatter::FormattedGrapheme, DisplayPosition};
use theme::Style;
use view::debugger::DebuggerInlineValue;

use super::Decoration;
use crate::ui::document::{LinePos, TextRenderer};

pub struct DebugValues<'a> {
    text: &'a Rope,
    values: &'a [DebuggerInlineValue],
    style: Style,
    hidden_line: Option<usize>,
    index: usize,
    ready: Option<usize>,
}

impl<'a> DebugValues<'a> {
    pub fn new(
        text: &'a Rope,
        values: &'a [DebuggerInlineValue],
        style: Style,
        hidden_line: Option<usize>,
    ) -> Self {
        Self {
            text,
            values,
            style,
            hidden_line,
            index: 0,
            ready: None,
        }
    }

    fn next_anchor(&self) -> usize {
        self.values.get(self.index).map_or(usize::MAX, |value| {
            line_end_char_index(&self.text.slice(..), value.line)
        })
    }
}

impl Decoration for DebugValues<'_> {
    fn reset_pos(&mut self, pos: usize) -> usize {
        self.ready = None;
        let line = self.text.char_to_line(pos.min(self.text.len_chars()));
        self.index = self.values.partition_point(|value| value.line < line);
        self.next_anchor()
    }

    fn decorate_grapheme(&mut self, _: &mut TextRenderer, grapheme: &FormattedGrapheme) -> usize {
        if grapheme.is_virtual() {
            return grapheme.char_idx;
        }
        self.ready = Some(self.index);
        self.index += 1;
        self.next_anchor()
    }

    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        off: DisplayPosition,
    ) -> DisplayPosition {
        let Some(value) = self.ready.take().and_then(|index| self.values.get(index)) else {
            return DisplayPosition::default();
        };
        if value.line != pos.doc_line
            || self.hidden_line == Some(value.line)
            || (pos.visual_line as usize) < renderer.offset.row
            || pos.visual_line as usize - renderer.offset.row >= renderer.viewport.height as usize
        {
            return DisplayPosition::default();
        }
        super::end_of_line::draw_label(renderer, pos, off, &value.text, self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{
        document::render_text,
        text_decorations::{code_lenses::CodeLensDecoration, DecorationManager},
    };
    use arc_swap::ArcSwap;
    use std::sync::Arc;
    use theme::Theme;
    use tui::{buffer::Buffer, layout::Rect};
    use view::{document::CodeLens, Document};

    fn render(
        text: &str,
        width: u16,
        wrap: bool,
        offset: DisplayPosition,
        hidden: Option<usize>,
    ) -> Buffer {
        let mut config = editor_config::Config::default();
        config.soft_wrap.enable = Some(wrap);
        let doc = Document::from(
            Rope::from_str(text),
            None,
            Arc::new(ArcSwap::from_pointee(config)),
            Arc::new(ArcSwap::from_pointee(language::syntax::Loader::default())),
        );
        let theme = Theme::default();
        let mut surface = Buffer::empty(Rect::new(0, 0, width, 6));
        let values = [DebuggerInlineValue {
            line: 0,
            text: "answer = 界界界界".into(),
        }];
        let lenses = [CodeLens {
            char_idx: 0,
            language_server_id: Default::default(),
            lens: lsp_client::lsp::CodeLens {
                range: Default::default(),
                data: None,
                command: Some(lsp_client::lsp::Command {
                    title: "Run".into(),
                    command: "run".into(),
                    arguments: None,
                }),
            },
        }];
        let mut decorations = DecorationManager::default();
        decorations.add_decoration(DebugValues::new(
            doc.text(),
            &values,
            Style::default(),
            hidden,
        ));
        decorations.add_decoration(CodeLensDecoration::new(
            doc.text(),
            &lenses,
            Style::default(),
            hidden,
        ));
        let mut renderer = TextRenderer::new(
            &mut surface,
            crate::ui::document::render_settings(&doc, &doc.config.load()),
            &theme,
            offset,
            view::graphics::Rect::new(0, 0, width, 6),
        );
        render_text(
            &mut renderer,
            doc.text().slice(..),
            0,
            &doc.text_format(width, Some(&theme)),
            &Default::default(),
            std::iter::empty(),
            vec![],
            &theme,
            decorations,
        );
        surface
    }

    fn row(surface: &Buffer, y: u16) -> String {
        (0..surface.area.width)
            .map(|x| surface[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn values_follow_source_at_eof_and_share_space_with_code_lenses() {
        for text in ["abc", "abc\nnext", "abc\r\nnext"] {
            let surface = render(text, 50, false, DisplayPosition::default(), None);
            let label = row(&surface, 0);
            assert!(label.starts_with("abc  answer = "), "{label}");
            assert_eq!(label.matches('界').count(), 4);
            assert!(label.contains("▶ Run"), "{label}");
            assert!(!row(&surface, 1).contains("answer"));
        }
    }

    #[test]
    fn values_clip_without_reflowing_wrapped_source_and_hide_during_completion() {
        let surface = render(
            "one two three four five\nnext",
            20,
            true,
            DisplayPosition::default(),
            None,
        );
        assert!(!row(&surface, 0).contains("answer"));
        assert!(row(&surface, 1).contains("answer = "));
        assert!(row(&surface, 1).contains('…'));
        assert!(row(&surface, 2).starts_with("next"));
        let hidden = render("abc\nnext", 30, false, DisplayPosition::default(), Some(0));
        assert_eq!(row(&hidden, 0).trim_end(), "abc");
        let scrolled = render("abc\nnext", 20, false, DisplayPosition::new(0, 8), None);
        assert!(row(&scrolled, 0).contains("wer = "));
        assert!(!row(&scrolled, 1).contains("answer"));
        let past_value = render("abc", 20, false, DisplayPosition::new(0, 24), None);
        assert!(
            row(&past_value, 0).starts_with(" Run"),
            "{}",
            row(&past_value, 0)
        );
    }
}

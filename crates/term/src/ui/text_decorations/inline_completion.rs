use super::Decoration;
use crate::ui::document::{LinePos, TextRenderer};
use text_layout::doc_formatter::{FormattedGrapheme, TextFormat};
use text_layout::DisplayPosition;
use theme::Style;
use view::{
    annotations::inline_completion::{InlineCompletionLines, InlineCompletionRow},
    document::InlineCompletion,
};

/// Paint a formatted preview while keeping the insertion cursor at its anchor.
pub struct InlineCompletionDecoration<'a> {
    layout: InlineCompletionLines<'a>,
    style: Style,
}

impl<'a> InlineCompletionDecoration<'a> {
    pub fn new(
        completion: &'a InlineCompletion,
        style: Style,
        format: TextFormat,
        line_end: usize,
    ) -> Self {
        Self {
            layout: InlineCompletionLines::new(
                completion.cursor,
                &completion.lines,
                format,
                line_end,
            ),
            style,
        }
    }
    fn draw(
        renderer: &mut TextRenderer,
        row: Option<&InlineCompletionRow>,
        pos: DisplayPosition,
        style: Style,
        soft_wrap: bool,
    ) {
        // Clip to this view before narrowing coordinates or touching the shared surface.
        if pos.row < renderer.offset.row
            || pos.row - renderer.offset.row >= renderer.viewport.height as usize
            || pos.col >= renderer.offset.col + renderer.viewport.width as usize
        {
            return;
        }
        let start = pos.col.saturating_sub(renderer.offset.col);
        let width = renderer.viewport.width as usize - start;
        let x = renderer.viewport.x + start as u16;
        renderer.set_string_truncated(
            x,
            pos.row as u16,
            &" ".repeat(width),
            width,
            |_| style,
            false,
            false,
        );
        if let Some(row) = row {
            // The shared formatter has already expanded tabs and wrapped Unicode graphemes.
            let col = row.col.saturating_sub(renderer.offset.col);
            if col < renderer.viewport.width as usize {
                use editor_core::unicode::{
                    segmentation::UnicodeSegmentation, width::UnicodeWidthStr,
                };
                let mut logical_col = row.col;
                let visible: String = row
                    .text
                    .graphemes(true)
                    .filter_map(|g| {
                        let from = logical_col;
                        logical_col += g.width();
                        if from >= renderer.offset.col {
                            Some(g.to_owned())
                        } else if logical_col > renderer.offset.col {
                            Some(" ".repeat(logical_col - renderer.offset.col))
                        } else {
                            None
                        }
                    })
                    .collect();
                renderer.set_string_truncated(
                    renderer.viewport.x + col as u16,
                    pos.row as u16,
                    &visible,
                    renderer.viewport.width as usize - col,
                    |_| style,
                    !soft_wrap,
                    false,
                );
            }
        }
    }
}

impl Decoration for InlineCompletionDecoration<'_> {
    fn reset_pos(&mut self, pos: usize) -> usize {
        self.layout.reset_pos(pos)
    }
    fn decorate_grapheme(&mut self, _: &mut TextRenderer, grapheme: &FormattedGrapheme) -> usize {
        self.layout.process_anchor(grapheme)
    }
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        off: DisplayPosition,
    ) -> DisplayPosition {
        let Some(block) = self.layout.block(pos.visual_line as usize, off) else {
            return DisplayPosition::new(0, 0);
        };
        for (position, row) in block.rows() {
            Self::draw(renderer, row, position, self.style, block.soft_wrap);
        }
        DisplayPosition::new(block.reserved_rows(), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_swap::ArcSwap;
    use editor_core::Rope;
    use language::syntax;
    use std::sync::Arc;

    #[test]
    fn inline_completion_painting_clips_to_the_viewport() {
        let doc = view::Document::from(
            Rope::from_str(""),
            None,
            Arc::new(ArcSwap::from_pointee(editor_config::Config::default())),
            Arc::new(syntax::Loader::default()),
        );
        let theme =
            theme::Theme::from(toml::from_str::<toml::Value>("\"ui.text\" = \"white\"").unwrap());
        let mut surface = tui::buffer::Buffer::empty(tui::layout::Rect::new(0, 0, 20, 8));
        for cell in &mut surface.content {
            cell.set_symbol("#");
        }
        let row = InlineCompletionRow {
            col: 0,
            text: "ghost".into(),
        };
        let mut renderer = TextRenderer::new(
            &mut surface,
            crate::ui::document::render_settings(&doc, &doc.config.load()),
            &theme,
            DisplayPosition::new(3, 0),
            view::graphics::Rect::new(0, 1, 20, 2),
        );
        InlineCompletionDecoration::draw(
            &mut renderer,
            Some(&row),
            DisplayPosition::new(2, 0),
            Style::default(),
            false,
        );
        InlineCompletionDecoration::draw(
            &mut renderer,
            Some(&row),
            DisplayPosition::new(5, 0),
            Style::default(),
            false,
        );
        assert!(surface.content.iter().all(|cell| cell.symbol() == "#"));
    }
}

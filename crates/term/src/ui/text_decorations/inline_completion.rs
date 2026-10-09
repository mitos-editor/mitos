use super::Decoration;
use crate::ui::document::{LinePos, TextRenderer};
use editor_core::{
    doc_formatter::{FormattedGrapheme, TextFormat},
    Position,
};
use view::{
    document::{InlineCompletion, InlineCompletionRow},
    theme::Style,
};

/// Paint a formatted preview while keeping the insertion cursor at its anchor.
pub struct InlineCompletionDecoration<'a> {
    completion: &'a InlineCompletion,
    style: Style,
    format: TextFormat,
    line_end: usize,
    anchor: Option<Position>,
    rows: Vec<InlineCompletionRow>,
    end_row: Option<usize>,
    rendered: bool,
}

impl<'a> InlineCompletionDecoration<'a> {
    pub fn new(
        completion: &'a InlineCompletion,
        style: Style,
        format: TextFormat,
        line_end: usize,
    ) -> Self {
        Self {
            completion,
            style,
            format,
            line_end,
            anchor: None,
            rows: Vec::new(),
            end_row: None,
            rendered: false,
        }
    }
    fn draw(
        renderer: &mut TextRenderer,
        row: Option<&InlineCompletionRow>,
        pos: Position,
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
        self.anchor = None;
        self.rows.clear();
        self.end_row = None;
        self.rendered = false;
        if pos <= self.completion.cursor {
            self.completion.cursor
        } else {
            usize::MAX
        }
    }
    fn decorate_grapheme(&mut self, _: &mut TextRenderer, grapheme: &FormattedGrapheme) -> usize {
        if grapheme.is_virtual() {
            return grapheme.char_idx;
        }
        if grapheme.char_idx == self.completion.cursor {
            self.anchor = Some(grapheme.visual_pos);
            self.rows = self
                .completion
                .layout(&self.format, grapheme.visual_pos.col);
        }
        if grapheme.char_idx >= self.line_end {
            self.end_row = Some(grapheme.visual_pos.row);
            usize::MAX
        } else {
            self.line_end
        }
    }
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        off: Position,
    ) -> Position {
        let Some(anchor) = self.anchor else {
            return Position::new(0, 0);
        };
        if self.rendered {
            return Position::new(0, 0);
        }
        let row = pos.visual_line as usize;
        let relative = row.saturating_sub(anchor.row);
        Self::draw(
            renderer,
            self.rows.get(relative),
            Position::new(row, if relative == 0 { anchor.col } else { 0 }),
            self.style,
            self.format.soft_wrap,
        );
        if self.end_row != Some(row) {
            return Position::new(0, 0);
        }
        self.rendered = true;
        let existing_rows = relative + 1;
        for (i, preview) in self.rows.iter().skip(existing_rows).enumerate() {
            Self::draw(
                renderer,
                Some(preview),
                Position::new(row + off.row + i, 0),
                self.style,
                self.format.soft_wrap,
            );
        }
        Position::new(self.rows.len().saturating_sub(existing_rows), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_swap::ArcSwap;
    use editor_core::{syntax, Rope};
    use std::sync::Arc;

    #[test]
    fn inline_completion_painting_clips_to_the_viewport() {
        let doc = view::Document::from(
            Rope::from_str(""),
            None,
            Arc::new(ArcSwap::from_pointee(view::config::Config::default())),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        let theme = view::theme::Theme::from(
            toml::from_str::<toml::Value>("\"ui.text\" = \"white\"").unwrap(),
        );
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
            &doc,
            &theme,
            Position::new(3, 0),
            view::graphics::Rect::new(0, 1, 20, 2),
        );
        InlineCompletionDecoration::draw(
            &mut renderer,
            Some(&row),
            Position::new(2, 0),
            Style::default(),
            false,
        );
        InlineCompletionDecoration::draw(
            &mut renderer,
            Some(&row),
            Position::new(5, 0),
            Style::default(),
            false,
        );
        assert!(surface.content.iter().all(|cell| cell.symbol() == "#"));
    }
}

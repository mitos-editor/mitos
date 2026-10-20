use std::cmp::min;
use text_layout::DisplayPosition;

use editor_config::{IndentGuidesConfig, WhitespaceConfig, WhitespaceRenderValue};
use editor_core::graphemes::Grapheme;
use editor_core::highlight::{HighlightEvent, OverlayHighlights};
use editor_core::line_ending::line_without_line_ending;
use editor_core::str_utils::char_to_byte_idx;
use editor_core::RopeSlice;
use ratatui_core::buffer::Buffer as Surface;
use text_layout::doc_formatter::{
    DocumentFormatter, FormattedGrapheme, GraphemeSource, TextFormat,
};
use text_layout::text_annotations::TextAnnotations;
use text_layout::visual_offset_from_block;
use theme::Style;
use theme::Theme;
use ui_core::graphics::Rect;

use crate::DecorationManager;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct LinePos {
    /// Indicates whether the given visual line
    /// is the first visual line of the given document line
    pub first_visual_line: bool,
    /// The line index of the document line that contains the given visual line
    pub doc_line: usize,
    /// Vertical offset from the top of the inner view area
    pub visual_line: u16,
    /// The given visual line is the last visual line of the document line
    pub is_last_visual_line: bool,
}

/// Paint text using resolved syntax styles at monotonically increasing character positions.
/// An empty stream uses the theme's base text style. Virtual text uses its own highlight.
#[allow(clippy::too_many_arguments)]
pub fn render_text(
    renderer: &mut TextRenderer,
    text: RopeSlice<'_>,
    anchor: usize,
    text_fmt: &TextFormat,
    text_annotations: &TextAnnotations,
    syntax_styles: impl Iterator<Item = (usize, Style)>,
    overlay_highlights: Vec<editor_core::highlight::OverlayHighlights>,
    theme: &Theme,
    mut decorations: DecorationManager,
) {
    let row_off = visual_offset_from_block(text, anchor, anchor, text_fmt, text_annotations)
        .0
        .row;

    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text, text_fmt, text_annotations, anchor);
    let mut syntax_styles = syntax_styles.peekable();
    let mut syntax_style = renderer.text_style;
    let mut overlay_highlighter = OverlayHighlighter::new(overlay_highlights, theme);

    let mut last_line_pos = LinePos {
        first_visual_line: false,
        doc_line: usize::MAX,
        visual_line: u16::MAX,
        is_last_visual_line: true,
    };
    let mut last_line_end = 0;
    let mut is_in_indent_area = true;
    let mut last_line_indent_level = 0;
    let mut reached_view_top = false;
    let mut trailing_line = usize::MAX;
    let mut trailing_start = 0;

    loop {
        let Some(mut grapheme) = formatter.next() else {
            break;
        };

        // skip any graphemes on visual lines before the block start
        if grapheme.visual_pos.row < row_off {
            continue;
        }
        grapheme.visual_pos.row -= row_off;
        if !reached_view_top {
            decorations.prepare_for_rendering(grapheme.char_idx);
            reached_view_top = true;
        }

        // if the end of the viewport is reached stop rendering
        if grapheme.visual_pos.row as u16 >= renderer.viewport.height + renderer.offset.row as u16 {
            last_line_pos.is_last_visual_line = last_line_pos.doc_line != grapheme.line_idx;
            break;
        }

        // apply decorations before rendering a new line
        if grapheme.visual_pos.row as u16 != last_line_pos.visual_line {
            if last_line_pos.doc_line == grapheme.line_idx {
                last_line_pos.is_last_visual_line = false;
            }

            // we initiate doc_line with usize::MAX because no file
            // can reach that size (memory allocations are limited to isize::MAX)
            // initially there is no "previous" line (so doc_line is set to usize::MAX)
            // in that case we don't need to draw indent guides/virtual text
            if last_line_pos.doc_line != usize::MAX {
                // draw indent guides for the last line
                renderer.draw_indent_guides(last_line_indent_level, last_line_pos.visual_line);
                is_in_indent_area = true;
                decorations.render_virtual_lines(renderer, last_line_pos, last_line_end)
            }
            last_line_pos = LinePos {
                first_visual_line: grapheme.line_idx != last_line_pos.doc_line,
                doc_line: grapheme.line_idx,
                visual_line: grapheme.visual_pos.row as u16,
                is_last_visual_line: true,
            };
            decorations.decorate_line(renderer, last_line_pos);
        }

        // acquire the correct grapheme style
        while syntax_styles
            .peek()
            .is_some_and(|(pos, _)| *pos <= grapheme.char_idx)
        {
            syntax_style = syntax_styles.next().unwrap().1;
        }
        while grapheme.char_idx >= overlay_highlighter.pos {
            overlay_highlighter.advance();
        }

        let grapheme_style = if let GraphemeSource::VirtualText { highlight } = grapheme.source {
            let mut style = renderer.text_style;
            if let Some(highlight) = highlight {
                style = style.patch(theme.highlight(highlight));
            }
            GraphemeStyle {
                syntax_style: style,
                overlay_style: Style::default(),
            }
        } else {
            GraphemeStyle {
                syntax_style,
                overlay_style: overlay_highlighter.style,
            }
        };
        decorations.decorate_grapheme(renderer, &grapheme);

        // Classify against the document line, not the visible row: a viewport
        // edge or soft wrap does not turn interior whitespace into trailing text.
        if renderer.classify_trailing && trailing_line != grapheme.line_idx {
            trailing_line = grapheme.line_idx;
            let line = line_without_line_ending(&text, trailing_line);
            let count = line
                .chars_at(line.len_chars())
                .reversed()
                .take_while(|ch| matches!(ch, ' ' | '\t' | '\u{00a0}' | '\u{202f}'))
                .count();
            trailing_start = text.line_to_char(trailing_line) + line.len_chars() - count;
        }
        let trailing = renderer.classify_trailing
            && !grapheme.is_virtual()
            && !grapheme.source.is_eof()
            && grapheme.char_idx >= trailing_start;
        let grapheme_width = renderer.draw_grapheme(
            &grapheme,
            grapheme_style,
            trailing,
            &mut last_line_indent_level,
            &mut is_in_indent_area,
            grapheme.visual_pos,
        );
        last_line_end = grapheme.visual_pos.col + grapheme_width;
        if !is_in_indent_area
            && grapheme.raw != Grapheme::Newline
            && grapheme.visual_pos.col >= renderer.offset.col + renderer.viewport.width as usize
        {
            last_line_end += formatter.skip_ascii(decorations.next_anchor());
        }
    }

    renderer.draw_indent_guides(last_line_indent_level, last_line_pos.visual_line);
    decorations.render_virtual_lines(renderer, last_line_pos, last_line_end)
}

// Presentation belongs to the terminal renderer; editor-config only owns policy.
#[derive(Debug)]
struct WhitespacePalette {
    space: String,
    nbsp: String,
    nnbsp: String,
    tab: String,
    newline: String,
}

impl WhitespacePalette {
    fn new(config: &WhitespaceConfig, tab_width: usize, trailing: bool) -> Self {
        let visible = |mode| {
            mode == WhitespaceRenderValue::All
                || (trailing && mode == WhitespaceRenderValue::Trailing)
        };
        let chars = &config.characters;
        let character = |mode, ch: char| {
            if visible(mode) {
                ch.to_string()
            } else {
                " ".into()
            }
        };
        Self {
            space: character(config.render.space(), chars.space),
            nbsp: character(config.render.nbsp(), chars.nbsp),
            nnbsp: character(config.render.nnbsp(), chars.nnbsp),
            tab: if visible(config.render.tab()) {
                std::iter::once(chars.tab)
                    .chain(std::iter::repeat_n(chars.tabpad, tab_width - 1))
                    .collect()
            } else {
                " ".repeat(tab_width)
            },
            // Newline markers are controlled independently; a newline is not trailing whitespace.
            newline: character(config.render.newline(), chars.newline),
        }
    }
}

/// Resolved document settings needed to paint text, independent of document ownership.
pub struct RenderSettings<'a> {
    pub whitespace: &'a WhitespaceConfig,
    pub indent_guides: &'a IndentGuidesConfig,
    /// Tab width in cells. Must be nonzero.
    pub tab_width: usize,
    /// Indentation width in cells. Must be nonzero and fit in `u16`.
    pub indent_width: usize,
}

#[derive(Debug)]
pub struct TextRenderer<'a> {
    surface: &'a mut Surface,
    text_style: Style,
    whitespace_style: Style,
    trailing_whitespace_style: Style,
    trailing_whitespace: WhitespacePalette,
    classify_trailing: bool,
    indent_guide_char: String,
    indent_guide_style: Style,
    newline: String,
    nbsp: String,
    nnbsp: String,
    space: String,
    tab: String,
    virtual_tab: String,
    indent_width: u16,
    starting_indent: usize,
    draw_indent_guides: bool,
    pub viewport: Rect,
    pub offset: DisplayPosition,
}

struct GraphemeStyle {
    syntax_style: Style,
    overlay_style: Style,
}

impl<'a> TextRenderer<'a> {
    pub fn new(
        surface: &'a mut Surface,
        settings: RenderSettings<'_>,
        theme: &Theme,
        offset: DisplayPosition,
        viewport: Rect,
    ) -> TextRenderer<'a> {
        let tab_width = settings.tab_width;
        let regular = WhitespacePalette::new(settings.whitespace, tab_width, false);
        let trailing_whitespace = WhitespacePalette::new(settings.whitespace, tab_width, true);

        let text_style = theme.get("ui.text");

        let indent_width = settings.indent_width as u16;

        TextRenderer {
            surface,
            indent_guide_char: settings.indent_guides.character.into(),
            newline: regular.newline,
            nbsp: regular.nbsp,
            nnbsp: regular.nnbsp,
            space: regular.space,
            tab: regular.tab,
            virtual_tab: " ".repeat(tab_width),
            trailing_whitespace,
            classify_trailing: {
                let render = settings.whitespace.render;
                let modes = [render.space(), render.tab(), render.nbsp(), render.nnbsp()];
                modes.contains(&WhitespaceRenderValue::Trailing)
                    || (theme
                        .try_get_exact("ui.virtual.trailing_whitespace")
                        .is_some()
                        && modes.contains(&WhitespaceRenderValue::All))
            },
            trailing_whitespace_style: theme
                .try_get_exact("ui.virtual.trailing_whitespace")
                .unwrap_or_else(|| theme.get("ui.virtual.whitespace")),
            whitespace_style: theme.get("ui.virtual.whitespace"),
            indent_width,
            starting_indent: offset.col / indent_width as usize
                + !offset.col.is_multiple_of(indent_width as usize) as usize
                + settings.indent_guides.skip_levels as usize,
            indent_guide_style: text_style.patch(
                theme
                    .try_get("ui.virtual.indent-guide")
                    .unwrap_or_else(|| theme.get("ui.virtual.whitespace")),
            ),
            text_style,
            draw_indent_guides: settings.indent_guides.render,
            viewport,
            offset,
        }
    }
    /// Draws a single `grapheme` at the current render position with a specified `style`.
    pub fn draw_decoration_grapheme(
        &mut self,
        grapheme: Grapheme,
        mut style: Style,
        mut row: u16,
        col: u16,
    ) -> bool {
        if (row as usize) < self.offset.row
            || row >= self.viewport.height
            || col >= self.viewport.width
        {
            return false;
        }
        row -= self.offset.row as u16;
        // TODO is it correct to apply the whitspace style to all unicode white spaces?
        if grapheme.is_whitespace() {
            style = style.patch(self.whitespace_style);
        }

        let grapheme = match grapheme {
            Grapheme::Tab { width } => {
                let grapheme_tab_width = char_to_byte_idx(&self.virtual_tab, width);
                &self.virtual_tab[..grapheme_tab_width]
            }
            Grapheme::Other { ref g } if g == "\u{00A0}" => " ",
            Grapheme::Other { ref g } => g,
            Grapheme::Newline => " ",
        };

        self.surface.set_string(
            self.viewport.x + col,
            self.viewport.y + row,
            grapheme,
            style,
        );
        true
    }

    /// Draws a single `grapheme` at the current render position with a specified `style`.
    fn draw_grapheme(
        &mut self,
        grapheme: &FormattedGrapheme,
        grapheme_style: GraphemeStyle,
        trailing: bool,
        last_indent_level: &mut usize,
        is_in_indent_area: &mut bool,
        mut position: DisplayPosition,
    ) -> usize {
        if position.row < self.offset.row {
            return 0;
        }
        position.row -= self.offset.row;
        let cut_off_start = self.offset.col.saturating_sub(position.col);
        let is_whitespace = grapheme.is_whitespace();
        let is_virtual = grapheme.is_virtual();
        let trailing = trailing && is_whitespace && !matches!(grapheme.raw, Grapheme::Newline);

        // TODO is it correct to apply the whitespace style to all unicode white spaces?
        let mut style = grapheme_style.syntax_style;
        if is_whitespace {
            style = style.patch(if trailing {
                self.trailing_whitespace_style
            } else {
                self.whitespace_style
            });
        }
        style = style.patch(grapheme_style.overlay_style);

        let width = grapheme.width();
        let mut is_tab = false;
        let palette = &self.trailing_whitespace;
        let space = if is_virtual {
            " "
        } else if trailing {
            &palette.space
        } else {
            &self.space
        };
        let nbsp = if is_virtual {
            " "
        } else if trailing {
            &palette.nbsp
        } else {
            &self.nbsp
        };
        let nnbsp = if is_virtual {
            " "
        } else if trailing {
            &palette.nnbsp
        } else {
            &self.nnbsp
        };
        let tab = if is_virtual {
            &self.virtual_tab
        } else if trailing {
            &palette.tab
        } else {
            &self.tab
        };
        let grapheme = match grapheme.raw {
            Grapheme::Tab { width } => {
                is_tab = true;
                let grapheme_tab_width = char_to_byte_idx(tab, width);
                &tab[..grapheme_tab_width]
            }
            // TODO special rendering for other whitespaces?
            Grapheme::Other { ref g } if g == " " && !grapheme.source.is_eof() => space,
            Grapheme::Other { ref g } if g == "\u{00A0}" => nbsp,
            Grapheme::Other { ref g } if g == "\u{202F}" => nnbsp,
            Grapheme::Other { ref g } => g,
            Grapheme::Newline => &self.newline,
        };

        let in_bounds = self.column_in_bounds(position.col, width);

        if in_bounds {
            let x = self.viewport.x + (position.col - self.offset.col) as u16;
            let y = self.viewport.y + position.row as u16;
            if is_tab {
                // A tab expands to `width` single-column cells; writing them
                // individually keeps background styles (selection, cursorline)
                // across the whole tab and avoids the redraw diff clipping
                // `render-whitespace` pads. A single `set_grapheme` would pack
                // them into one wide cell and leave the rest unstyled.
                self.surface.set_tab(x, y, grapheme, style);
            } else {
                self.surface.set_grapheme(x, y, grapheme, width, style);
            }
        } else if is_tab && position.col < self.offset.col + self.viewport.width as usize {
            // Tabs are individual cells, so either edge can clip their marker/padding.
            let start = cut_off_start.min(width);
            let end = width
                .min((self.offset.col + self.viewport.width as usize).saturating_sub(position.col));
            if start < end {
                let from = char_to_byte_idx(grapheme, start);
                let to = char_to_byte_idx(grapheme, end);
                self.surface.set_tab(
                    self.viewport.x + position.col.saturating_sub(self.offset.col) as u16,
                    self.viewport.y + position.row as u16,
                    &grapheme[from..to],
                    style,
                );
            }
        } else if cut_off_start != 0 && cut_off_start < width {
            // partially on screen
            let rect = Rect::new(
                self.viewport.x,
                self.viewport.y + position.row as u16,
                (width - cut_off_start) as u16,
                1,
            );
            self.surface.set_style(rect, style);
        }
        if *is_in_indent_area && !is_whitespace {
            *last_indent_level = position.col;
            *is_in_indent_area = false;
        }

        width
    }

    pub fn column_in_bounds(&self, colum: usize, width: usize) -> bool {
        self.offset.col <= colum && colum + width <= self.offset.col + self.viewport.width as usize
    }

    /// Overlay indentation guides ontop of a rendered line
    /// The indentation level is computed in `draw_lines`.
    /// Therefore this function must always be called afterwards.
    fn draw_indent_guides(&mut self, indent_level: usize, mut row: u16) {
        if !self.draw_indent_guides || self.offset.row > row as usize {
            return;
        }
        row -= self.offset.row as u16;

        // Don't draw indent guides outside of view
        let end_indent = min(
            indent_level,
            // Add indent_width - 1 to round up, since the first visible
            // indent might be a bit after offset.col
            self.offset.col + self.viewport.width as usize + (self.indent_width as usize - 1),
        ) / self.indent_width as usize;

        for i in self.starting_indent..end_indent {
            let x = (self.viewport.x as usize + (i * self.indent_width as usize) - self.offset.col)
                as u16;
            let y = self.viewport.y + row;
            debug_assert!(self.surface.in_bounds(x, y));
            self.surface
                .set_string(x, y, &self.indent_guide_char, self.indent_guide_style);
        }
    }

    pub fn set_string(&mut self, x: u16, y: u16, string: &str, style: Style) {
        if (y as usize) < self.offset.row {
            return;
        }
        let y = y - self.offset.row as u16;
        self.surface
            .set_string(x, y + self.viewport.y, string, style)
    }

    pub fn set_stringn(&mut self, x: u16, y: u16, string: &str, width: usize, style: Style) {
        if (y as usize) < self.offset.row {
            return;
        }
        let y = y - self.offset.row as u16;
        self.surface
            .set_stringn(x, y + self.viewport.y, string, width, style);
    }

    /// Sets the style of an area **within the text viewport* this accounts
    /// both for the renderers vertical offset and its viewport
    pub fn set_style(&mut self, mut area: Rect, style: Style) {
        let offset = self.offset.row as u16;
        if area.y < offset {
            area.height = area.height.saturating_sub(offset - area.y);
            area.y = offset;
        }
        area.y = area.y - offset + self.viewport.y;
        self.surface.set_style(area, style);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_string_truncated(
        &mut self,
        x: u16,
        y: u16,
        string: &str,
        width: usize,
        style: impl Fn(usize) -> Style, // Map a grapheme's string offset to a style
        ellipsis: bool,
        truncate_start: bool,
    ) -> (u16, u16) {
        if (y as usize) < self.offset.row {
            return (x, y);
        }
        let y = y - self.offset.row as u16;
        self.surface.set_string_truncated(
            x,
            y + self.viewport.y,
            string,
            width,
            style,
            ellipsis,
            truncate_start,
        )
    }
}

struct OverlayHighlighter<'t> {
    inner: editor_core::highlight::OverlayHighlighter,
    pos: usize,
    theme: &'t Theme,
    style: Style,
}

impl<'t> OverlayHighlighter<'t> {
    fn new(overlays: Vec<OverlayHighlights>, theme: &'t Theme) -> Self {
        let inner = editor_core::highlight::OverlayHighlighter::new(overlays);
        let mut highlighter = Self {
            inner,
            pos: 0,
            theme,
            style: Style::default(),
        };
        highlighter.update_pos();
        highlighter
    }

    fn update_pos(&mut self) {
        self.pos = self.inner.next_event_offset();
    }

    fn advance(&mut self) {
        let (event, highlights) = self.inner.advance();
        let base = match event {
            HighlightEvent::Refresh => Style::default(),
            HighlightEvent::Push => self.style,
        };

        self.style = highlights.fold(base, |acc, highlight| {
            acc.patch(self.theme.highlight(highlight))
        });
        self.update_pos();
    }
}
use ui_core::buffer::BufferExt as _;

#[cfg(test)]
mod whitespace_tests {
    use super::*;
    use editor_core::Rope;
    use ratatui_core::style::Color;

    fn render(text: &str, settings: &str, width: u16, wrap: bool, offset: usize) -> Surface {
        render_with_annotations(text, settings, width, wrap, offset, &[])
    }

    fn render_with_annotations(
        text: &str,
        settings: &str,
        width: u16,
        wrap: bool,
        offset: usize,
        inline: &[text_layout::text_annotations::InlineAnnotation],
    ) -> Surface {
        let mut config: editor_config::Config = toml::from_str(settings).unwrap();
        config.soft_wrap.enable = Some(wrap);
        config.soft_wrap.wrap_indicator = Some(String::new());
        config.soft_wrap.max_wrap = Some(0);
        config.whitespace.characters.tabpad = '·';
        let text = Rope::from_str(text);
        let theme = Theme::from(toml::from_str::<toml::Value>(
            "\"ui.virtual.whitespace\" = { fg = 'blue' }\n\"ui.virtual.trailing_whitespace\" = { fg = 'red' }",
        ).unwrap());
        let viewport = Rect::new(0, 0, width, 8);
        let mut surface = Surface::empty(viewport);
        let mut renderer = TextRenderer::new(
            &mut surface,
            RenderSettings {
                whitespace: &config.whitespace,
                indent_guides: &config.indent_guides,
                tab_width: 4,
                indent_width: 4,
            },
            &theme,
            DisplayPosition::new(0, offset),
            viewport,
        );
        let mut annotations = TextAnnotations::default();
        annotations.add_inline_annotations(inline, None);
        render_text(
            &mut renderer,
            text.slice(..),
            0,
            &TextFormat {
                soft_wrap: wrap && width > 10,
                viewport_width: width,
                max_wrap: 0,
                max_indent_retain: 40.min(width * 2 / 5),
                ..TextFormat::default()
            },
            &annotations,
            std::iter::empty(),
            vec![],
            &theme,
            DecorationManager::default(),
        );
        surface
    }

    fn row(surface: &Surface, y: u16) -> String {
        (0..surface.area.width)
            .map(|x| surface[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn clipped_ascii_tails_preserve_following_lines_unicode_tabs_and_annotations() {
        use text_layout::text_annotations::InlineAnnotation;
        // Prefixes differ only beyond the right edge. A long tail must not hide
        // later lines or shift text following tabs and combining graphemes.
        let tail = "x".repeat(100_000);
        for suffix in ["\nnext line\n", "e\u{301}\tend\r\nnext\n", "界\nnext\n"] {
            let text = format!("start {tail}{suffix}");
            let long = render(&text, "", 20, false, 0);
            let short = render(
                &format!("start {}{suffix}", "x".repeat(100)),
                "",
                20,
                false,
                0,
            );
            assert_eq!(long, short);
            assert!(row(&long, 1).starts_with("next"));
        }
        let inline = [InlineAnnotation::new(tail.len() + 6, "hint\nvirtual")];
        let result =
            render_with_annotations(&format!("start {tail}\nnext"), "", 20, false, 0, &inline);
        assert!(row(&result, 1).contains("virtual"));
        assert!(row(&result, 2).contains("next"));
    }

    #[test]
    fn skipped_tails_still_visit_decoration_anchors() {
        use std::cell::Cell;
        struct Anchor<'a>(&'a Cell<usize>);
        impl crate::Decoration for Anchor<'_> {
            fn reset_pos(&mut self, _: usize) -> usize {
                50_000
            }
            fn decorate_grapheme(
                &mut self,
                _: &mut TextRenderer,
                grapheme: &FormattedGrapheme,
            ) -> usize {
                self.0.set(grapheme.visual_pos.col);
                usize::MAX
            }
        }
        let position = Cell::new(usize::MAX);
        let text = Rope::from(format!("{}\nnext", "x".repeat(100_000)));
        let config = editor_config::Config::default();
        let theme = Theme::from(toml::from_str::<toml::Value>("'ui.text' = 'white'").unwrap());
        let area = Rect::new(0, 0, 20, 3);
        let mut surface = Surface::empty(area);
        let mut renderer = TextRenderer::new(
            &mut surface,
            RenderSettings {
                whitespace: &config.whitespace,
                indent_guides: &config.indent_guides,
                tab_width: 4,
                indent_width: 4,
            },
            &theme,
            DisplayPosition::default(),
            area,
        );
        let mut decorations = DecorationManager::default();
        decorations.add_decoration(Anchor(&position));
        render_text(
            &mut renderer,
            text.slice(..),
            0,
            &TextFormat::default(),
            &TextAnnotations::default(),
            std::iter::empty(),
            vec![],
            &theme,
            decorations,
        );
        assert_eq!(position.get(), 50_000);
        assert!(row(&surface, 1).starts_with("next"));
    }

    #[test]
    fn skipped_tails_advance_styles_and_preserve_annotations() {
        use text_layout::text_annotations::InlineAnnotation;
        let text = Rope::from(format!("{}\nend", "x".repeat(100_000)));
        let config = editor_config::Config::default();
        let theme = Theme::from(toml::from_str::<toml::Value>("'ui.text' = 'white'").unwrap());
        let area = Rect::new(0, 0, 20, 3);
        let mut surface = Surface::empty(area);
        let mut renderer = TextRenderer::new(
            &mut surface,
            RenderSettings {
                whitespace: &config.whitespace,
                indent_guides: &config.indent_guides,
                tab_width: 4,
                indent_width: 4,
            },
            &theme,
            DisplayPosition::default(),
            area,
        );
        let inline = [InlineAnnotation::new(50_000, "hint")];
        let mut annotations = TextAnnotations::default();
        annotations.add_inline_annotations(&inline, None);
        let styles = [
            (0, Style::default().fg(theme::Color::Red)),
            (100_001, Style::default().fg(theme::Color::Blue)),
        ];
        render_text(
            &mut renderer,
            text.slice(..),
            0,
            &TextFormat::default(),
            &annotations,
            styles.into_iter(),
            vec![],
            &theme,
            DecorationManager::default(),
        );
        assert!(row(&surface, 1).starts_with("end"));
        assert_eq!(surface[(0, 0)].fg, Color::Red);
        assert_eq!(surface[(0, 1)].fg, Color::Blue);
    }

    #[test]
    fn syntax_styles_follow_character_positions_through_virtual_text_and_clipping() {
        use text_layout::text_annotations::InlineAnnotation;
        let text = Rope::from_str("é界x");
        let config = editor_config::Config::default();
        let theme = Theme::from(toml::from_str::<toml::Value>("\"ui.text\" = \"white\"").unwrap());
        let viewport = Rect::new(0, 0, 12, 1);
        let mut surface = Surface::empty(viewport);
        let mut renderer = TextRenderer::new(
            &mut surface,
            RenderSettings {
                whitespace: &config.whitespace,
                indent_guides: &config.indent_guides,
                tab_width: 4,
                indent_width: 4,
            },
            &theme,
            DisplayPosition::new(0, 1),
            viewport,
        );
        let inline = [InlineAnnotation::new(1, "hint")];
        let mut annotations = TextAnnotations::default();
        annotations.add_inline_annotations(&inline, None);
        let style = |color| Style::default().fg(color);
        render_text(
            &mut renderer,
            text.slice(..),
            0,
            &TextFormat::default(),
            &annotations,
            [
                (0, style(theme::Color::Red)),
                (1, style(theme::Color::Blue)),
                (2, style(theme::Color::Green)),
            ]
            .into_iter(),
            vec![],
            &theme,
            DecorationManager::default(),
        );
        assert_eq!(row(&surface, 0), "hint界 x     ");
        assert_eq!(surface[(0, 0)].fg, Color::White);
        assert_eq!(surface[(4, 0)].fg, Color::Blue);
        assert_eq!(surface[(6, 0)].fg, Color::Green);
    }

    #[test]
    fn trailing_spaces_and_unicode_before_crlf_and_eof() {
        let surface = render(
            "a b \u{00a0}\u{202f}\r\nc ",
            "[whitespace]\nrender = 'trailing'",
            12,
            false,
            0,
        );
        assert_eq!(row(&surface, 0), "a b·⍽␣      ");
        assert_eq!(row(&surface, 1), "c·          ");
        assert_eq!(surface[(1, 0)].fg, Color::Blue);
        assert_eq!(surface[(3, 0)].fg, Color::Red);
        // The formatter's synthetic EOF space must stay invisible.
        assert_eq!(surface[(2, 1)].symbol(), " ");
    }

    #[test]
    fn trailing_modes_remain_independent() {
        let surface = render(
            "a b\t \n",
            "[whitespace.render]\nspace = 'all'\ntab = 'trailing'\nnewline = 'all'",
            12,
            false,
            0,
        );
        assert_eq!(row(&surface, 0), "a·b→·⏎      ");
        let hidden = render("a \t\u{00a0}\n", "[whitespace.render]\nspace = 'trailing'\ntab = 'none'\nnbsp = 'none'\nnewline = 'trailing'", 12, false, 0);
        assert_eq!(row(&hidden, 0), "a·          ");
    }

    #[test]
    fn viewport_edge_does_not_make_interior_whitespace_trailing() {
        let surface = render("ab   z  ", "[whitespace]\nrender = 'trailing'", 5, false, 0);
        assert_eq!(row(&surface, 0), "ab   ");
        let scrolled = render("ab   z  ", "[whitespace]\nrender = 'trailing'", 5, false, 3);
        assert_eq!(row(&scrolled, 0), "  z··");
    }

    #[test]
    fn clipped_tabs_keep_their_actual_expansion_and_style() {
        let surface = render("a\t\t", "[whitespace]\nrender = 'trailing'", 5, false, 2);
        assert_eq!(row(&surface, 0), "··→··");
        for x in 0..5 {
            assert_eq!(surface[(x, 0)].fg, Color::Red);
        }
    }

    #[test]
    fn virtual_text_does_not_change_trailing_classification() {
        use text_layout::text_annotations::InlineAnnotation;
        let surface = render_with_annotations(
            "a  ",
            "[whitespace]\nrender = 'trailing'",
            12,
            false,
            0,
            &[InlineAnnotation::new(2, "hint ")],
        );
        assert_eq!(row(&surface, 0), "a·hint ·    ");
        assert_eq!(surface[(6, 0)].symbol(), " ");
        assert_eq!(surface[(7, 0)].fg, Color::Red);
    }

    #[test]
    fn soft_wrap_uses_the_document_line_end() {
        // Document::text_format enables soft wrap only for widths above ten.
        let surface = render(
            "abcdefghij   z  ",
            "[whitespace]\nrender = 'trailing'",
            12,
            true,
            0,
        );
        assert_eq!(row(&surface, 0), "abcdefghij  ");
        assert_eq!(row(&surface, 1), " z··        ");
        let trailing_run = render(
            "abcdefghij    ",
            "[whitespace]\nrender = 'trailing'",
            12,
            true,
            0,
        );
        assert_eq!(row(&trailing_run, 0), "abcdefghij··");
        assert_eq!(row(&trailing_run, 1), "··          ");
    }
}

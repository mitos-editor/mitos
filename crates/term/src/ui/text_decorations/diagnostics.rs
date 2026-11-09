use text_layout::DisplayPosition;

use ::diagnostics::Diagnostic;
use ::diagnostics::Severity;
use editor_config::{DiagnosticFilter, InlineDiagnosticsConfig};
use text_layout::doc_formatter::FormattedGrapheme;
use view::annotations::diagnostics::InlineDiagnosticAccumulator;

use theme::icons::ICONS;
use theme::Style;
use theme::Theme;
use view::Document;

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

#[derive(Debug)]
struct Styles {
    hint: Style,
    info: Style,
    warning: Style,
    error: Style,
}

impl Styles {
    fn new(theme: &Theme) -> Styles {
        Styles {
            hint: theme.get("ui.diagnostic.inline.hint"),
            info: theme.get("ui.diagnostic.inline.info"),
            warning: theme.get("ui.diagnostic.inline.warning"),
            error: theme.get("ui.diagnostic.inline.error"),
        }
    }

    fn severity_style(&self, severity: Severity) -> Style {
        match severity {
            Severity::Hint => self.hint,
            Severity::Info => self.info,
            Severity::Warning => self.warning,
            Severity::Error => self.error,
        }
    }
}

pub struct InlineDiagnostics<'a> {
    state: InlineDiagnosticAccumulator<'a>,
    eol_diagnostics: DiagnosticFilter,
    styles: Styles,
    icons: bool,
}

impl<'a> InlineDiagnostics<'a> {
    pub fn new(
        doc: &'a Document,
        theme: &Theme,
        cursor: usize,
        config: InlineDiagnosticsConfig,
        eol_diagnostics: DiagnosticFilter,
        icons: bool,
    ) -> Self {
        InlineDiagnostics {
            state: InlineDiagnosticAccumulator::new(cursor, doc.diagnostics(), config),
            styles: Styles::new(theme),
            eol_diagnostics,
            icons,
        }
    }
}

impl InlineDiagnostics<'_> {
    fn draw_eol_diagnostic(
        &self,
        renderer: &mut TextRenderer,
        diag: &Diagnostic,
        row: u16,
        col: usize,
    ) -> u16 {
        let style = self.styles.severity_style(diag.severity());
        let width = renderer.viewport.width;
        let start_col = (col - renderer.offset.col) as u16;
        let mut end_col = start_col;
        let mut draw_col = (col + 1) as u16;

        if self.icons {
            let icons = &*ICONS;
            let icon = match diag.severity() {
                Severity::Hint => icons.diagnostic().hint(),
                Severity::Info => icons.diagnostic().info(),
                Severity::Warning => icons.diagnostic().warning(),
                Severity::Error => icons.diagnostic().error(),
            };
            if !renderer.column_in_bounds(draw_col as usize, icon.width() as usize) {
                return 0;
            }
            renderer.set_string(
                renderer.viewport.x + draw_col,
                row,
                icon.glyph().as_str(),
                style,
            );
            draw_col += icon.width();
        }

        for line in diag.message.lines() {
            if !renderer.column_in_bounds(draw_col as usize, 1) {
                break;
            }

            (end_col, _) = renderer.set_string_truncated(
                renderer.viewport.x + draw_col,
                row,
                line,
                width.saturating_sub(draw_col) as usize,
                |_| style,
                true,
                false,
            );

            draw_col = end_col - renderer.viewport.x + 2; // double space between lines
        }

        end_col - start_col
    }
}

impl Decoration for InlineDiagnostics<'_> {
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: DisplayPosition,
    ) -> DisplayPosition {
        let col_off = self
            .state
            .eol_diagnostic(self.eol_diagnostics)
            .map_or(0, |diag| {
                self.draw_eol_diagnostic(renderer, diag, pos.visual_line, virt_off.col)
            });
        let first_row = pos.visual_line + virt_off.row as u16;
        let height = self.state.layout_virtual_lines(
            renderer.viewport.width,
            |grapheme, severity, position| {
                renderer.draw_decoration_grapheme(
                    grapheme,
                    self.styles.severity_style(severity),
                    first_row + position.row as u16,
                    position.col as u16,
                );
            },
        );
        DisplayPosition::new(height, col_off as usize)
    }

    fn reset_pos(&mut self, pos: usize) -> usize {
        self.state.reset_pos(pos)
    }

    fn skip_concealed_anchor(&mut self, conceal_end_char_idx: usize) -> usize {
        self.state.skip_concealed(conceal_end_char_idx)
    }

    fn decorate_grapheme(
        &mut self,
        renderer: &mut TextRenderer,
        grapheme: &FormattedGrapheme,
    ) -> usize {
        self.state
            .process_anchor(grapheme, renderer.viewport.width, renderer.offset.col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_swap::ArcSwap;
    use diagnostics::DiagnosticProvider;
    use editor_core::Rope;
    use std::sync::Arc;

    #[tokio::test]
    async fn reserved_diagnostic_rows_match_the_painted_block() {
        let mut config = editor_config::Config::default();
        config.diagnostics.inline.other_lines = DiagnosticFilter::Enable(Severity::Warning);
        let inline = config.diagnostics.inline.clone();
        let mut doc = Document::from(
            Rope::from_str("a\nz"),
            None,
            Arc::new(ArcSwap::from_pointee(config)),
            Arc::new(language::syntax::Loader::default()),
        );
        doc.replace_diagnostics(
            [Diagnostic {
                range: diagnostics::Range { start: 0, end: 1 },
                ends_at_word: false,
                starts_at_word: false,
                zero_width: false,
                line: 0,
                message: "x".repeat(48).into(),
                severity: Some(Severity::Warning),
                code: None,
                provider: DiagnosticProvider::Spelling,
                tags: Vec::new(),
                source: None,
                data: None,
            }],
            &[],
            None,
        );
        let mut view = view::View::new(
            doc.id(),
            editor_config::GutterConfig {
                layout: Vec::new(),
                ..Default::default()
            },
            event::Redraw::default(),
        );
        view.area = view::graphics::Rect::new(0, 0, 50, 10);
        doc.ensure_view_init(view.id);
        let theme = Theme::from(toml::from_str::<toml::Value>("\"ui.text\" = \"white\"").unwrap());
        let layout = view.layout(&doc, Some(&theme));
        let next_line = layout.display_position_at_pos(2).unwrap();
        let mut surface = tui::buffer::Buffer::empty(tui::layout::Rect::new(0, 0, 50, 10));
        let mut decorations = super::super::DecorationManager::default();
        decorations.add_decoration(InlineDiagnostics::new(
            &doc,
            &theme,
            0,
            inline,
            DiagnosticFilter::Disable,
            false,
        ));
        crate::ui::document::render_document_with_styles(
            &mut surface,
            &doc,
            &layout,
            Vec::new(),
            &theme,
            decorations,
            std::iter::empty(),
        );
        assert_eq!(
            next_line.row, 3,
            "two diagnostic rows must precede the next source line"
        );
        assert_eq!(
            surface[(2, 2)].symbol(),
            "x",
            "the second diagnostic row must survive source painting"
        );
        assert_eq!(surface[(0, 3)].symbol(), "z");
    }
}

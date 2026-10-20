use ::diagnostics::Diagnostic;
use editor_config::{DiagnosticFilter, InlineDiagnosticsConfig};
use text_layout::doc_formatter::{FormattedGrapheme, TextFormat};
use text_layout::softwrapped_dimensions;
use text_layout::text_annotations::LineAnnotation;
use text_layout::DisplayPosition;

pub fn prepare_inline_diagnostics(
    config: &InlineDiagnosticsConfig,
    width: u16,
    enable_cursor_line: bool,
) -> InlineDiagnosticsConfig {
    let mut config = config.clone();
    if width < config.min_diagnostic_width + config.prefix_len {
        config.cursor_line = DiagnosticFilter::Disable;
        config.other_lines = DiagnosticFilter::Disable;
    } else if !enable_cursor_line {
        config.cursor_line = config.cursor_line.min(config.other_lines);
    }
    config
}

pub fn max_diagnostic_start(config: &InlineDiagnosticsConfig, width: u16) -> u16 {
    width - config.min_diagnostic_width - config.prefix_len
}

pub fn diagnostic_text_format(
    config: &InlineDiagnosticsConfig,
    anchor_col: u16,
    width: u16,
) -> TextFormat {
    let width = if anchor_col > max_diagnostic_start(config, width) {
        config.min_diagnostic_width
    } else {
        width - anchor_col - config.prefix_len
    };

    TextFormat {
        soft_wrap: true,
        tab_width: 4,
        max_wrap: config.max_wrap.min(width / 4),
        max_indent_retain: 0,
        wrap_indicator: Box::from(""),
        wrap_indicator_highlight: None,
        viewport_width: width,
        soft_wrap_at_text_width: true,
    }
}

pub struct InlineDiagnosticAccumulator<'a> {
    idx: usize,
    diagnostics: &'a [Diagnostic],
    pub stack: Vec<(&'a Diagnostic, u16)>,
    pub config: InlineDiagnosticsConfig,
    cursor: usize,
    cursor_line: bool,
}

impl<'a> InlineDiagnosticAccumulator<'a> {
    /// Diagnostics must be ordered by their starting character position.
    pub fn new(
        cursor: usize,
        diagnostics: &'a [Diagnostic],
        config: InlineDiagnosticsConfig,
    ) -> Self {
        InlineDiagnosticAccumulator {
            idx: 0,
            diagnostics,
            stack: Vec::new(),
            config,
            cursor,
            cursor_line: false,
        }
    }

    pub fn reset_pos(&mut self, char_idx: usize) -> usize {
        self.idx = 0;
        self.clear();
        self.skip_concealed(char_idx)
    }

    pub fn skip_concealed(&mut self, conceal_end_char_idx: usize) -> usize {
        let diagnostics = &self.diagnostics[self.idx..];
        let idx = diagnostics.partition_point(|diag| diag.range.start < conceal_end_char_idx);
        self.idx += idx;
        self.next_anchor(conceal_end_char_idx)
    }

    pub fn next_anchor(&self, current_char_idx: usize) -> usize {
        let next_diag_start = self
            .diagnostics
            .get(self.idx)
            .map_or(usize::MAX, |diag| diag.range.start);
        if (current_char_idx..next_diag_start).contains(&self.cursor) {
            self.cursor
        } else {
            next_diag_start
        }
    }

    pub fn clear(&mut self) {
        self.cursor_line = false;
        self.stack.clear();
    }

    fn process_anchor_impl(
        &mut self,
        grapheme: &FormattedGrapheme,
        width: u16,
        horizontal_off: usize,
    ) -> bool {
        // TODO: doing the cursor tracking here works well but is somewhat
        // duplicate effort/tedious maybe centralize this somewhere?
        // In the DocFormatter?
        if grapheme.char_idx == self.cursor {
            self.cursor_line = true;
            if self
                .diagnostics
                .get(self.idx)
                .is_none_or(|diag| diag.range.start != grapheme.char_idx)
            {
                return false;
            }
        }

        let Some(anchor_col) = grapheme.visual_pos.col.checked_sub(horizontal_off) else {
            return true;
        };
        if anchor_col >= width as usize {
            return true;
        }

        for diag in &self.diagnostics[self.idx..] {
            if diag.range.start != grapheme.char_idx {
                break;
            }
            self.stack.push((diag, anchor_col as u16));
            self.idx += 1;
        }
        false
    }

    pub fn process_anchor(
        &mut self,
        grapheme: &FormattedGrapheme,
        width: u16,
        horizontal_off: usize,
    ) -> usize {
        if self.process_anchor_impl(grapheme, width, horizontal_off) {
            self.idx += self.diagnostics[self.idx..]
                .iter()
                .take_while(|diag| diag.range.start == grapheme.char_idx)
                .count();
        }
        self.next_anchor(grapheme.char_idx + 1)
    }

    pub fn filter(&self) -> DiagnosticFilter {
        if self.cursor_line {
            self.config.cursor_line
        } else {
            self.config.other_lines
        }
    }

    pub fn compute_line_diagnostics(&mut self) {
        let filter = if self.cursor_line {
            self.cursor_line = false;
            self.config.cursor_line
        } else {
            self.config.other_lines
        };
        let DiagnosticFilter::Enable(filter) = filter else {
            self.stack.clear();
            return;
        };
        self.stack.retain(|(diag, _)| diag.severity() >= filter);
        self.stack.truncate(self.config.max_diagnostics)
    }

    pub fn has_multi(&self, width: u16) -> bool {
        self.stack
            .last()
            .is_some_and(|&(_, anchor)| anchor > max_diagnostic_start(&self.config, width))
    }
}

pub(crate) struct InlineDiagnostics<'a> {
    state: InlineDiagnosticAccumulator<'a>,
    width: u16,
    horizontal_off: usize,
}

impl<'a> InlineDiagnostics<'a> {
    #[allow(clippy::new_ret_no_self)]
    pub(crate) fn new(
        diagnostics: &'a [Diagnostic],
        cursor: usize,
        width: u16,
        horizontal_off: usize,
        config: InlineDiagnosticsConfig,
    ) -> Box<dyn LineAnnotation + 'a> {
        Box::new(InlineDiagnostics {
            state: InlineDiagnosticAccumulator::new(cursor, diagnostics, config),
            width,
            horizontal_off,
        })
    }
}

impl LineAnnotation for InlineDiagnostics<'_> {
    fn reset_pos(&mut self, char_idx: usize) -> usize {
        self.state.reset_pos(char_idx)
    }

    fn skip_concealed_anchors(&mut self, conceal_end_char_idx: usize) -> usize {
        self.state.skip_concealed(conceal_end_char_idx)
    }

    fn process_anchor(&mut self, grapheme: &FormattedGrapheme) -> usize {
        self.state
            .process_anchor(grapheme, self.width, self.horizontal_off)
    }

    fn insert_virtual_lines(
        &mut self,
        _line_end_char_idx: usize,
        _line_end_visual_pos: DisplayPosition,
        _doc_line: usize,
    ) -> DisplayPosition {
        self.state.compute_line_diagnostics();
        let multi = self.state.has_multi(self.width);
        let diagnostic_height: usize = self
            .state
            .stack
            .drain(..)
            .map(|(diag, anchor)| {
                let text_fmt = diagnostic_text_format(&self.state.config, anchor, self.width);
                softwrapped_dimensions(diag.message.trim().into(), &text_fmt).0
            })
            .sum();
        DisplayPosition::new(multi as usize + diagnostic_height, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics::{DiagnosticProvider, Severity};
    use editor_core::Rope;
    use text_layout::{doc_formatter::DocumentFormatter, text_annotations::TextAnnotations};

    fn rows(cursor: usize, horizontal_off: usize, config: InlineDiagnosticsConfig) -> usize {
        let text = Rope::from_str("ab\ncd");
        let diagnostics = [Diagnostic {
            range: diagnostics::Range { start: 0, end: 1 },
            ends_at_word: false,
            starts_at_word: false,
            zero_width: false,
            line: 0,
            message: "warning".into(),
            severity: Some(Severity::Warning),
            code: None,
            provider: DiagnosticProvider::Spelling,
            tags: Vec::new(),
            source: None,
            data: None,
        }];
        let mut annotations = TextAnnotations::default();
        annotations.add_line_annotation(InlineDiagnostics::new(
            &diagnostics,
            cursor,
            50,
            horizontal_off,
            config,
        ));
        DocumentFormatter::new_at_prev_checkpoint(
            text.slice(..),
            &TextFormat::default(),
            &annotations,
            0,
        )
        .last()
        .unwrap()
        .visual_pos
        .row + 1
    }

    #[test]
    fn diagnostic_rows_follow_cursor_filters_and_horizontal_visibility() {
        let config = InlineDiagnosticsConfig {
            cursor_line: DiagnosticFilter::Disable,
            other_lines: DiagnosticFilter::Enable(Severity::Warning),
            ..Default::default()
        };
        assert_eq!(rows(0, 0, config.clone()), 2);
        assert_eq!(rows(3, 0, config.clone()), 3);
        assert_eq!(rows(3, 4, config), 2);
        let config = InlineDiagnosticsConfig {
            cursor_line: DiagnosticFilter::Enable(Severity::Error),
            other_lines: DiagnosticFilter::Enable(Severity::Error),
            ..Default::default()
        };
        assert_eq!(rows(0, 0, config), 2);
    }
}

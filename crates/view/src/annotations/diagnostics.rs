use editor_config::{DiagnosticFilter, InlineDiagnosticsConfig};
use editor_core::{Diagnostic, Position};
use text_layout::doc_formatter::{FormattedGrapheme, TextFormat};
use text_layout::softwrapped_dimensions;
use text_layout::text_annotations::LineAnnotation;

use crate::Document;

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
    doc: &'a Document,
    pub stack: Vec<(&'a Diagnostic, u16)>,
    pub config: InlineDiagnosticsConfig,
    cursor: usize,
    cursor_line: bool,
}

impl<'a> InlineDiagnosticAccumulator<'a> {
    pub fn new(cursor: usize, doc: &'a Document, config: InlineDiagnosticsConfig) -> Self {
        InlineDiagnosticAccumulator {
            idx: 0,
            doc,
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
        let diagnostics = &self.doc.diagnostics[self.idx..];
        let idx = diagnostics.partition_point(|diag| diag.range.start < conceal_end_char_idx);
        self.idx += idx;
        self.next_anchor(conceal_end_char_idx)
    }

    pub fn next_anchor(&self, current_char_idx: usize) -> usize {
        let next_diag_start = self
            .doc
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
                .doc
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

        for diag in &self.doc.diagnostics[self.idx..] {
            if diag.range.start != grapheme.char_idx {
                break;
            }
            self.stack.push((diag, anchor_col as u16));
            self.idx += 1;
        }
        false
    }

    pub fn proccess_anchor(
        &mut self,
        grapheme: &FormattedGrapheme,
        width: u16,
        horizontal_off: usize,
    ) -> usize {
        if self.process_anchor_impl(grapheme, width, horizontal_off) {
            self.idx += self.doc.diagnostics[self.idx..]
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
        doc: &'a Document,
        cursor: usize,
        width: u16,
        horizontal_off: usize,
        config: InlineDiagnosticsConfig,
    ) -> Box<dyn LineAnnotation + 'a> {
        Box::new(InlineDiagnostics {
            state: InlineDiagnosticAccumulator::new(cursor, doc, config),
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
            .proccess_anchor(grapheme, self.width, self.horizontal_off)
    }

    fn insert_virtual_lines(
        &mut self,
        _line_end_char_idx: usize,
        _line_end_visual_pos: Position,
        _doc_line: usize,
    ) -> Position {
        self.state.compute_line_diagnostics();
        let multi = self.state.has_multi(self.width);
        let diagostic_height: usize = self
            .state
            .stack
            .drain(..)
            .map(|(diag, anchor)| {
                let text_fmt = diagnostic_text_format(&self.state.config, anchor, self.width);
                softwrapped_dimensions(diag.message.trim().into(), &text_fmt).0
            })
            .sum();
        Position::new(multi as usize + diagostic_height, 0)
    }
}

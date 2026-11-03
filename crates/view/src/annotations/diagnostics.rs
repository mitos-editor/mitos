use ::diagnostics::{Diagnostic, Severity};
use editor_config::{DiagnosticFilter, InlineDiagnosticsConfig};
use editor_core::graphemes::Grapheme;
use std::cmp::Ordering;
use text_layout::doc_formatter::{DocumentFormatter, FormattedGrapheme, TextFormat};
use text_layout::text_annotations::{LineAnnotation, TextAnnotations};
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

fn max_diagnostic_start(config: &InlineDiagnosticsConfig, width: u16) -> u16 {
    width - config.min_diagnostic_width - config.prefix_len
}

fn diagnostic_text_format(
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

#[derive(Clone)]
pub struct InlineDiagnosticAccumulator<'a> {
    idx: usize,
    diagnostics: &'a [Diagnostic],
    stack: Vec<(&'a Diagnostic, u16)>,
    config: InlineDiagnosticsConfig,
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

    fn filter(&self) -> DiagnosticFilter {
        if self.cursor_line {
            self.config.cursor_line
        } else {
            self.config.other_lines
        }
    }

    fn compute_line_diagnostics(&mut self) {
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

    /// Choose an end-of-line label before filtering the inline block.
    pub fn eol_diagnostic(&self, eol_filter: DiagnosticFilter) -> Option<&Diagnostic> {
        let DiagnosticFilter::Enable(eol_filter) = eol_filter else {
            return None;
        };
        let inline_filter = self.filter();
        self.stack
            .iter()
            .map(|(diag, _)| *diag)
            .filter(|diag| diag.severity() >= eol_filter)
            .filter(|diag| match inline_filter {
                DiagnosticFilter::Enable(filter) => diag.severity() < filter,
                DiagnosticFilter::Disable => true,
            })
            .max_by_key(|diag| diag.severity)
    }

    /// Stream a positioned diagnostic block. A measurement visitor may ignore glyphs;
    /// painting uses the same wrapping, grouping and connector geometry.
    pub fn layout_virtual_lines(
        &mut self,
        width: u16,
        draw: impl FnMut(Grapheme, Severity, DisplayPosition),
    ) -> usize {
        self.compute_line_diagnostics();
        let mut layout = DiagnosticLayout {
            draw,
            width,
            row: 0,
            config: &self.config,
        };
        layout.draw_multi_diagnostics(&mut self.stack);
        layout.draw_diagnostics(&mut self.stack);
        layout.row
    }
}

const BL_CORNER: &str = "┘";
const TR_CORNER: &str = "┌";
const BR_CORNER: &str = "└";
const STACK: &str = "├";
const MULTI: &str = "┴";
const HOR_BAR: &str = "─";
const VER_BAR: &str = "│";

struct DiagnosticLayout<'a, F> {
    draw: F,
    width: u16,
    row: usize,
    config: &'a InlineDiagnosticsConfig,
}

impl<F: FnMut(Grapheme, Severity, DisplayPosition)> DiagnosticLayout<'_, F> {
    fn draw_decoration(&mut self, g: &'static str, severity: Severity, col: u16) {
        self.draw_decoration_at(g, severity, col, self.row)
    }

    fn draw_decoration_at(&mut self, g: &'static str, severity: Severity, col: u16, row: usize) {
        (self.draw)(
            Grapheme::new_decoration(g),
            severity,
            DisplayPosition::new(row, col as usize),
        );
    }

    fn draw_diagnostic(&mut self, diag: &Diagnostic, col: u16, next_severity: Option<Severity>) {
        let severity = diag.severity();
        let (sym, sym_severity) = if let Some(next_severity) = next_severity {
            (STACK, next_severity.max(severity))
        } else {
            (BR_CORNER, severity)
        };
        self.draw_decoration(sym, sym_severity, col);
        for i in 0..self.config.prefix_len {
            self.draw_decoration(HOR_BAR, severity, col + i + 1);
        }

        let text_col = col + self.config.prefix_len + 1;
        let text_fmt = diagnostic_text_format(self.config, text_col, self.width);
        let annotations = TextAnnotations::default();
        let formatter = DocumentFormatter::new_at_prev_checkpoint(
            diag.message.trim().into(),
            &text_fmt,
            &annotations,
            0,
        );
        let mut last_row = 0;
        for grapheme in formatter {
            last_row = grapheme.visual_pos.row;
            (self.draw)(
                grapheme.raw,
                severity,
                DisplayPosition::new(
                    self.row + grapheme.visual_pos.row,
                    text_col as usize + grapheme.visual_pos.col,
                ),
            );
        }
        self.row += 1;
        // height is last_row + 1 and extra_rows is height - 1
        let extra_lines = last_row;
        if let Some(next_severity) = next_severity {
            for _ in 0..extra_lines {
                self.draw_decoration(VER_BAR, next_severity, col);
                self.row += 1;
            }
        } else {
            self.row += extra_lines;
        }
    }

    fn draw_multi_diagnostics(&mut self, stack: &mut Vec<(&Diagnostic, u16)>) {
        let Some(&(last_diag, last_anchor)) = stack.last() else {
            return;
        };
        let start = max_diagnostic_start(self.config, self.width);

        if last_anchor <= start {
            return;
        }
        let mut severity = last_diag.severity();
        let mut last_anchor = last_anchor;
        self.draw_decoration(BL_CORNER, severity, last_anchor);
        let mut stacked_diagnostics = 1;
        for &(diag, anchor) in stack.iter().rev().skip(1) {
            let sym = match anchor.cmp(&start) {
                Ordering::Less => break,
                Ordering::Equal => STACK,
                Ordering::Greater => MULTI,
            };
            stacked_diagnostics += 1;
            severity = severity.max(diag.severity());
            let old_severity = severity;
            if anchor == last_anchor && severity == old_severity {
                continue;
            }
            for col in (anchor + 1)..last_anchor {
                self.draw_decoration(HOR_BAR, old_severity, col)
            }
            self.draw_decoration(sym, severity, anchor);
            last_anchor = anchor;
        }

        // if no diagnostic anchor was found exactly at the start of the
        // diagnostic text  draw an upwards corner and ensure the last piece
        // of the line is not missing
        if last_anchor != start {
            for col in (start + 1)..last_anchor {
                self.draw_decoration(HOR_BAR, severity, col)
            }
            self.draw_decoration(TR_CORNER, severity, start)
        }
        self.row += 1;
        let stacked_diagnostics = &stack[stack.len() - stacked_diagnostics..];

        for (i, (diag, _)) in stacked_diagnostics.iter().rev().enumerate() {
            let next_severity = stacked_diagnostics[..stacked_diagnostics.len() - i - 1]
                .iter()
                .map(|(diag, _)| diag.severity())
                .max();
            self.draw_diagnostic(diag, start, next_severity);
        }

        stack.truncate(stack.len() - stacked_diagnostics.len());
    }

    fn draw_diagnostics(&mut self, stack: &mut Vec<(&Diagnostic, u16)>) {
        let mut stack = stack.drain(..).rev().peekable();
        let mut last_anchor = self.width;
        while let Some((diag, anchor)) = stack.next() {
            if anchor != last_anchor {
                for row in 0..self.row {
                    self.draw_decoration_at(VER_BAR, diag.severity(), anchor, row);
                }
            }
            let next_severity = stack.peek().and_then(|&(diag, next_anchor)| {
                (next_anchor == anchor).then_some(diag.severity())
            });
            self.draw_diagnostic(diag, anchor, next_severity);
            last_anchor = anchor;
        }
    }
}

#[derive(Clone)]
pub(crate) struct InlineDiagnostics<'a> {
    state: InlineDiagnosticAccumulator<'a>,
    width: u16,
    horizontal_off: usize,
}

impl<'a> InlineDiagnostics<'a> {
    pub(crate) fn new(
        diagnostics: &'a [Diagnostic],
        cursor: usize,
        width: u16,
        horizontal_off: usize,
        config: InlineDiagnosticsConfig,
    ) -> Self {
        InlineDiagnostics {
            state: InlineDiagnosticAccumulator::new(cursor, diagnostics, config),
            width,
            horizontal_off,
        }
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
        let height = self.state.layout_virtual_lines(self.width, |_, _, _| {});
        DisplayPosition::new(height, 0)
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

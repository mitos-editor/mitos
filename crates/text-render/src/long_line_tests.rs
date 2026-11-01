use crate::{render_text, Decoration, DecorationManager, LinePos, RenderSettings, TextRenderer};
use editor_core::{highlight::OverlayHighlights, Rope};
use ratatui_core::buffer::Buffer;
use std::{cell::RefCell, rc::Rc};
use text_layout::{
    doc_formatter::{FormattedGrapheme, TextFormat},
    text_annotations::{LineAnnotation, TextAnnotations},
    DisplayPosition,
};
use theme::{Color, Style, Theme};
use ui_core::graphics::Rect;

struct EmptyLine;
impl LineAnnotation for EmptyLine {
    fn insert_virtual_lines(&mut self, _: usize, _: DisplayPosition, _: usize) -> DisplayPosition {
        DisplayPosition::default()
    }
}

struct Anchors {
    positions: Vec<usize>,
    index: usize,
    visited: Rc<RefCell<Vec<(usize, usize)>>>,
}
impl Decoration for Anchors {
    fn reset_pos(&mut self, pos: usize) -> usize {
        self.index = self.positions.partition_point(|anchor| *anchor < pos);
        self.positions
            .get(self.index)
            .copied()
            .unwrap_or(usize::MAX)
    }
    fn decorate_grapheme(&mut self, _: &mut TextRenderer, g: &FormattedGrapheme) -> usize {
        self.visited
            .borrow_mut()
            .push((g.char_idx, g.visual_pos.col));
        self.index += 1;
        self.positions
            .get(self.index)
            .copied()
            .unwrap_or(usize::MAX)
    }
}

fn paint(
    source: &str,
    force_slow: bool,
    positions: Vec<usize>,
    offset: usize,
    wrap: bool,
) -> (Buffer, Vec<(usize, usize)>) {
    let text = Rope::from_str(source);
    let mut config: editor_config::Config =
        toml::from_str("[whitespace]\nrender = 'trailing'\n[indent-guides]\nrender = true")
            .unwrap();
    config.whitespace.characters.tabpad = '·';
    let theme = Theme::default();
    let viewport = Rect::new(0, 0, 30, 8);
    let mut surface = Buffer::empty(viewport);
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
    if force_slow {
        annotations.add_line_annotation(Box::new(EmptyLine));
    }
    let visited = Rc::new(RefCell::new(Vec::new()));
    let mut decorations = DecorationManager::default();
    decorations.add_decoration(Anchors {
        positions,
        index: 0,
        visited: visited.clone(),
    });
    // Line callbacks remain installed, as in the actual editor.
    decorations.add_decoration(|_: &mut TextRenderer, _: LinePos| {});
    render_text(
        &mut renderer,
        text.slice(..),
        0,
        &TextFormat {
            soft_wrap: wrap,
            viewport_width: 30,
            ..TextFormat::default()
        },
        &annotations,
        [
            (0, Style::default().fg(Color::Red)),
            (100, Style::default().fg(Color::Green)),
            (
                text.len_chars().saturating_sub(5),
                Style::default().fg(Color::Yellow),
            ),
        ]
        .into_iter(),
        vec![OverlayHighlights::Heterogenous {
            highlights: vec![(Theme::rgb_highlight(40, 100, 200), 500..700)],
        }],
        &theme,
        decorations,
    );
    let visits = visited.borrow().clone();
    (surface, visits)
}

#[test]
fn skipping_tail_matches_slow_rendering_and_preserves_decorations_and_next_rows() {
    for first in [
        "a".repeat(5_000),
        " \t".repeat(2_000),
        format!("{}text", " \t".repeat(2_000)),
        format!("{}界", "a".repeat(5_000)),
        format!("{}\x1b", "a".repeat(5_000)),
        format!("é😀界{}", " \tx".repeat(2_000)),
    ] {
        for ending in ["", "\n", "\r\n"] {
            let source = format!("{first}{ending}next visible line\nlast line");
            let end = first.chars().count();
            for hooks in [vec![], vec![end], vec![200, end]] {
                for offset in [0, 5, 100] {
                    assert_eq!(
                        paint(&source, false, hooks.clone(), offset, false),
                        paint(&source, true, hooks.clone(), offset, false)
                    );
                }
            }
        }
    }
    let source = format!("{}\nnext", "a".repeat(5_000));
    assert_eq!(
        paint(&source, false, vec![100], 0, true),
        paint(&source, true, vec![100], 0, true)
    );
}

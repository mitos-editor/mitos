//! Compare full traversal with viewport-tail skipping in the same optimized binary.
use std::{hint::black_box, time::Instant};

use editor_core::Rope;
use ratatui_core::buffer::Buffer;
use text_layout::{doc_formatter::TextFormat, text_annotations::TextAnnotations, DisplayPosition};
use text_render::{
    render_text, Decoration, DecorationManager, LinePos, RenderSettings, TextRenderer,
};
use theme::Theme;
use ui_core::graphics::Rect;

// A decoration without the opt-in retains the original full traversal.
struct FullTraversal;
impl Decoration for FullTraversal {}

// Keep the default opt-out: this consumer needs the exact line-end column,
// including the invisible tail. Install it in both paths for the fallback case.
struct WidthDependentDecoration;
impl Decoration for WidthDependentDecoration {
    fn render_virt_lines(
        &mut self,
        _renderer: &mut TextRenderer,
        _pos: LinePos,
        virt_off: DisplayPosition,
    ) -> DisplayPosition {
        black_box(virt_off.col);
        DisplayPosition::new(0, 0)
    }
}

fn main() {
    let config = editor_config::Config::default();
    let theme = Theme::from(toml::from_str::<toml::Value>("").unwrap());
    let viewport = Rect::new(0, 0, 120, 40);
    for fixture in ["ascii", "mixed-unicode", "width-dependent-decoration"] {
        for length in [100_000, 1_000_000] {
            let first_line = if fixture == "ascii" {
                "x".repeat(length)
            } else {
                // Include tabs, wide characters, combining marks, and emoji in the
                // visible prefix and hidden tail while keeping character counts equal.
                "x界e\u{301}\t🙂".chars().cycle().take(length).collect()
            };
            let text = Rope::from_str(&format!(
                "{}\n{}",
                first_line,
                "following line\n".repeat(39)
            ));
            for offset in [0, 10_000] {
                let draw = |skip: bool| {
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
                    let mut decorations = DecorationManager::default();
                    if fixture == "width-dependent-decoration" {
                        decorations.add_decoration(WidthDependentDecoration);
                    }
                    if !skip {
                        decorations.add_decoration(FullTraversal);
                    }
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
                    surface
                };
                assert_eq!(draw(false), draw(true));
                let mut times = Vec::new();
                for skip in [false, true] {
                    let started = Instant::now();
                    let iterations = 30;
                    for _ in 0..iterations {
                        black_box(draw(black_box(skip)));
                    }
                    times.push(started.elapsed().as_secs_f64() * 1_000.0 / iterations as f64);
                }
                println!("fixture={fixture} length={length} viewport=120x40 offset={offset} full_ms={:.4} candidate_ms={:.4} speedup={:.1}x", times[0], times[1], times[0] / times[1]);
            }
        }
    }
}

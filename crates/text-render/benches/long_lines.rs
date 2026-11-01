use editor_config::Config;
use editor_core::Rope;
use ratatui_core::buffer::Buffer;
use std::{hint::black_box, time::Instant};
use text_layout::{doc_formatter::TextFormat, text_annotations::TextAnnotations, DisplayPosition};
use text_render::{render_text, DecorationManager, RenderSettings, TextRenderer};
fn main() {
    let config = Config::default();
    let theme = theme::Theme::default();
    let viewport = ui_core::graphics::Rect::new(0, 0, 120, 40);
    for count in [1_000, 100_000, 1_000_000] {
        for unicode_tail in [false, true] {
            let mut source = "a".repeat(count);
            if unicode_tail {
                source.push('界');
            }
            source.push_str("\nfollowing visible line\n");
            let text = Rope::from_str(&source);
            let mut surface = Buffer::empty(viewport);
            for offset in [0, 1_000] {
                let mut run = || {
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
                    render_text(
                        &mut renderer,
                        text.slice(..),
                        0,
                        &TextFormat {
                            soft_wrap: false,
                            viewport_width: 120,
                            ..Default::default()
                        },
                        &TextAnnotations::default(),
                        std::iter::empty(),
                        vec![],
                        &theme,
                        DecorationManager::default(),
                    );
                    black_box(&surface);
                };
                for _ in 0..3 {
                    run();
                }
                let mut samples: Vec<_> = (0..21)
                    .map(|_| {
                        let t = Instant::now();
                        run();
                        t.elapsed().as_secs_f64() * 1000.0
                    })
                    .collect();
                samples.sort_by(f64::total_cmp);
                println!(
                    "chars={count} unicode_tail={unicode_tail} offset={offset} median_ms={:.6}",
                    samples[10]
                );
            }
        }
    }
}

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use editor_core::Rope;
use ratatui_core::{buffer::Buffer, layout::Rect};
use std::hint::black_box;
use text_layout::{doc_formatter::TextFormat, text_annotations::TextAnnotations, DisplayPosition};
use text_render::{render_text, DecorationManager, RenderSettings, TextRenderer};

fn long_lines(c: &mut Criterion) {
    let config = editor_config::Config::default();
    let theme = theme::Theme::from(toml::from_str::<toml::Value>("'ui.text' = 'white'").unwrap());
    let area = Rect::new(0, 0, 120, 40);
    let mut group = c.benchmark_group("unwrapped_ascii_line");
    for length in [1_000, 100_000, 1_000_000] {
        let text = Rope::from("x".repeat(length));
        let mut buffer = Buffer::empty(area);
        group.bench_with_input(BenchmarkId::from_parameter(length), &text, |b, text| {
            b.iter(|| {
                let mut renderer = TextRenderer::new(
                    &mut buffer,
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
                render_text(
                    &mut renderer,
                    black_box(text.slice(..)),
                    0,
                    &TextFormat::default(),
                    &TextAnnotations::default(),
                    std::iter::empty(),
                    vec![],
                    &theme,
                    DecorationManager::default(),
                );
            })
        });
    }
    group.finish();
}
criterion_group!(benches, long_lines);
criterion_main!(benches);

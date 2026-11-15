//! Component workloads for viewport preparation and text drawing.
//! Run with `cargo bench --locked -p term --bench rendering` and compiled grammars.
use std::{hint::black_box, time::Instant};

use editor_core::{
    highlight::{Highlight, OverlayHighlighter, OverlayHighlights},
    Rope,
};
use text_layout::{
    doc_formatter::TextFormat,
    text_annotations::{Conceal, TextAnnotations},
    DisplayPosition,
};
use text_render::{render_text, DecorationManager, RenderSettings, TextRenderer};
use theme::Theme;
use tui::buffer::Buffer;
use view::{
    graphics::Rect,
    view::{ViewLayout, ViewPosition},
};

fn median_us(mut operation: impl FnMut(), iterations: usize) -> f64 {
    for _ in 0..3 {
        operation();
    }
    let mut samples = Vec::new();
    for _ in 0..7 {
        let started = Instant::now();
        for _ in 0..iterations {
            operation();
        }
        samples.push(started.elapsed().as_secs_f64() * 1e6 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    samples[3]
}

fn visible_ranges() {
    for count in [1_000, 10_000, 100_000] {
        let text = Rope::from_str(&format!(
            "{}{}",
            "h{\nx\n}\n".repeat(count),
            "tail\n".repeat(80)
        ));
        let folds: Vec<_> = (0..count)
            .map(|n| Conceal::new(n * 7 + 2..n * 7 + 5))
            .collect();
        for (position, anchor) in [("top", 0), ("bottom", count * 7)] {
            let mut annotations = TextAnnotations::default();
            annotations.set_conceals(&folds);
            let layout = ViewLayout {
                text: text.slice(..),
                viewport: Rect::new(0, 0, 120, 40),
                offset: ViewPosition {
                    anchor,
                    ..Default::default()
                },
                format: TextFormat::default(),
                annotations,
            };
            let us = median_us(
                || {
                    black_box(layout.visible_byte_ranges());
                },
                20,
            );
            println!("visible_ranges folds={count} position={position} median_us={us:.3}");
        }
    }
}

fn long_lines() {
    let config = editor_config::Config::default();
    let theme = Theme::default();
    let viewport = Rect::new(0, 0, 120, 40);
    for length in [100_000, 1_000_000] {
        let text = Rope::from_str(&format!(
            "{}\n{}h{{\nx\n}}\n",
            "x".repeat(length),
            "next\n".repeat(80)
        ));
        let base = length + 1 + 80 * 5;
        let folds = [Conceal::new(base + 2..base + 5)];
        let draw = |fold: bool| {
            let mut surface = Buffer::empty(viewport);
            let mut annotations = TextAnnotations::default();
            if fold {
                annotations.set_conceals(&folds);
            }
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
                viewport,
            );
            render_text(
                &mut renderer,
                text.slice(..),
                0,
                &TextFormat::default(),
                &annotations,
                std::iter::empty(),
                vec![],
                &theme,
                DecorationManager::default(),
            );
            surface
        };
        assert_eq!(draw(false), draw(true));
        for fold in [false, true] {
            let us = median_us(
                || {
                    black_box(draw(fold));
                },
                5,
            );
            println!("draw length={length} offscreen_fold={fold} median_us={us:.3}");
        }
    }
}

fn overlays() {
    for count in [1_000, 10_000, 100_000] {
        let ranges: Vec<_> = (0..count).map(|n| n * 4..n * 4 + 1).collect();
        for seek in [false, true] {
            let us = median_us(
                || {
                    let mut cursor = OverlayHighlighter::new([OverlayHighlights::Homogeneous {
                        highlight: Highlight::new(0),
                        ranges: ranges.clone(),
                    }]);
                    if seek {
                        black_box(cursor.advance_to(count * 4).count());
                    } else {
                        while cursor.next_event_offset() <= count * 4 {
                            black_box(cursor.advance().count());
                        }
                    }
                },
                20,
            );
            println!("overlay_prefix ranges={count} seek={seek} median_us={us:.3}");
        }
    }
}

fn rainbow_brackets() {
    let resources =
        loader::syntax::Resources::new(vec![
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime")
        ]);
    let loader = language::loader::default_lang_loader(resources);
    let json = loader.language_for_name("json").unwrap();
    for count in [1_000, 10_000, 100_000] {
        let header = "{\n\"items\": [\n";
        let content = format!(
            "{}{}{{\"a\":1}}\n],\n\"tail\":1\n}}\n",
            header,
            "{\"a\":1},\n".repeat(count)
        );
        let end = content.rfind(']').unwrap();
        let text = Rope::from_str(&content);
        let syntax = language::Syntax::new_with_timeout(
            text.slice(..),
            json,
            &loader,
            std::time::Duration::from_secs(20),
        )
        .unwrap();
        let visible = [0..header.len() - 1, end..text.len()];
        for folded in [false, true] {
            let ranges = if folded {
                visible.to_vec()
            } else {
                std::iter::once(0..text.len()).collect()
            };
            let mut emitted = 0;
            let us = median_us(
                || {
                    let overlay = syntax.rainbow_highlights(
                        text.slice(..),
                        6,
                        &loader,
                        ranges
                            .iter()
                            .map(|range| range.start as u32..range.end as u32),
                    );
                    let OverlayHighlights::Heterogenous { highlights } = &overlay else {
                        unreachable!()
                    };
                    emitted = highlights.len();
                    black_box(overlay);
                },
                3,
            );
            println!("rainbow hidden_objects={count} folded={folded} emitted={emitted} median_us={us:.3}");
        }
    }
}

fn main() {
    visible_ranges();
    long_lines();
    overlays();
    rainbow_brackets();
}

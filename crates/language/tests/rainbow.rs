use std::ops::Range;

use editor_core::{
    highlight::{Highlight, OverlayHighlights},
    Rope,
};
use language::Syntax;

fn spans(overlay: OverlayHighlights) -> Vec<(Highlight, Range<usize>)> {
    let OverlayHighlights::Heterogenous { highlights } = overlay else {
        panic!("rainbow brackets use individual highlights")
    };
    highlights
}

#[test]
fn visible_segments_preserve_enclosing_depth_and_injected_languages() {
    let loader = language::loader::default_lang_loader(loader::syntax::Resources::default());
    for (name, before, hidden, after) in [
        (
            "json",
            "{\"label\": \"é界\", \"items\": [",
            "{\"nested\": [1,2]}, {\"nested\": [[3]]}",
            "], \"after\": [[]]}",
        ),
        (
            "html",
            "<script>function outer() {",
            "call({ values: [1, 2] });",
            "after([[]]);}</script>",
        ),
    ] {
        let text = Rope::from_str(&format!("{before}{hidden}{after}"));
        let syntax = Syntax::new(
            text.slice(..),
            loader.language_for_name(name).unwrap(),
            &loader,
        )
        .unwrap();
        let ranges = [0..before.len(), before.len() + hidden.len()..text.len()];
        let full = spans(syntax.rainbow_highlights(
            text.slice(..),
            6,
            &loader,
            std::iter::once(0..text.len() as u32),
        ));
        assert!(!full.is_empty(), "{name} must exercise bracket coloring");
        let expected: Vec<_> = full
            .into_iter()
            .filter(|(_, span)| {
                ranges
                    .iter()
                    .any(|range| range.contains(&text.char_to_byte_idx(span.start)))
            })
            .collect();
        let actual = spans(
            syntax.rainbow_highlights(
                text.slice(..),
                6,
                &loader,
                ranges
                    .iter()
                    .map(|range| range.start as u32..range.end as u32),
            ),
        );
        assert_eq!(
            actual, expected,
            "{name}: colors after a fold must keep their depth"
        );
    }
}

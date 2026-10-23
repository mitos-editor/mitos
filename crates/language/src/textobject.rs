//! Structural selections from syntax trees and matched pairs.
use crate::{surround, syntax, Syntax};
use editor_core::{
    graphemes::next_grapheme_boundary, textobject::TextObject, Range, RopeSlice, SmallVec,
};

pub fn textobject_pair_surround(
    syntax: Option<&Syntax>,
    slice: RopeSlice,
    range: Range,
    textobject: TextObject,
    ch: char,
    count: usize,
) -> Range {
    textobject_pair_surround_impl(syntax, slice, range, textobject, Some(ch), count)
}

pub fn textobject_pair_surround_closest(
    syntax: Option<&Syntax>,
    slice: RopeSlice,
    range: Range,
    textobject: TextObject,
    count: usize,
) -> Range {
    textobject_pair_surround_impl(syntax, slice, range, textobject, None, count)
}

fn textobject_pair_surround_impl(
    syntax: Option<&Syntax>,
    slice: RopeSlice,
    range: Range,
    textobject: TextObject,
    ch: Option<char>,
    count: usize,
) -> Range {
    let pair_pos = match ch {
        Some(ch) => surround::find_nth_pairs_pos(syntax, slice, ch, range, count),
        None => surround::find_nth_closest_pairs_pos(syntax, slice, range, count),
    };
    pair_pos
        .map(|(anchor, head)| match textobject {
            TextObject::Inside => {
                if anchor < head {
                    Range::new(next_grapheme_boundary(slice, anchor), head)
                } else {
                    Range::new(anchor, next_grapheme_boundary(slice, head))
                }
            }
            TextObject::Around => {
                if anchor < head {
                    Range::new(anchor, next_grapheme_boundary(slice, head))
                } else {
                    Range::new(next_grapheme_boundary(slice, anchor), head)
                }
            }
            TextObject::Movement => unreachable!(),
        })
        .unwrap_or(range)
}

/// Transform the given range to select text objects based on tree-sitter.
/// `object_name` is a query capture base name like "function", "class", etc.
/// `slice_tree` is the tree-sitter node corresponding to given text slice.
pub fn textobject_treesitter(
    slice: RopeSlice,
    range: Range,
    textobject: TextObject,
    object_name: &str,
    syntax: &Syntax,
    loader: &syntax::Loader,
    _count: usize,
) -> Range {
    let byte_pos = slice.char_to_byte_idx(range.cursor(slice));
    let capture_name = format!("{}.{}", object_name, textobject); // eg. function.inner
    let len = slice.len();

    // try the innermost layer first, then fall back to its parents
    // an injected language might not define the requested textobject
    let layers: Vec<_> = syntax
        .layers_for_byte_range(byte_pos as u32, byte_pos as u32)
        .collect();
    layers
        .into_iter()
        .rev()
        .find_map(|layer| {
            let layer = syntax.layer(layer);
            let root = layer.tree()?.root_node();
            let node = loader
                .textobject_query(layer.language)?
                .capture_nodes(&capture_name, &root, slice)?
                .filter(|node| node.byte_range().contains(&byte_pos))
                .min_by_key(|node| node.byte_range().len())?;

            let start_byte = node.start_byte();
            let end_byte = node.end_byte();
            if start_byte >= len || end_byte >= len {
                return None;
            }

            let start_char = slice.byte_to_char_idx(start_byte);
            let end_char = slice.byte_to_char_idx(end_byte);

            Some(Range::new(start_char, end_char))
        })
        .unwrap_or(range)
}

/// Collect query captures fully contained in `range`, preserving its direction.
/// A document without a syntax tree has no query-backed text objects.
pub fn textobject_treesitter_ranges(
    text: RopeSlice,
    range: Range,
    objtype: TextObject,
    obj_name: &str,
    syntax: Option<&Syntax>,
    loader: &syntax::Loader,
) -> SmallVec<[Range; 1]> {
    let Some(syntax) = syntax else {
        return SmallVec::new();
    };

    let from = text.char_to_byte_idx(range.from()) as u32;
    let to = text.char_to_byte_idx(range.to()) as u32;
    // Syntax layer ranges use an inclusive end while editor ranges use an exclusive end.
    let end = if to > from { to - 1 } else { to };
    let layer = syntax.layer_for_byte_range(from, end);
    let root = syntax.tree_for_byte_range(from, end).root_node();
    let Some(textobject_query) = loader.textobject_query(syntax.layer(layer).language) else {
        return SmallVec::new();
    };

    let capture_name = format!("{obj_name}.{objtype}");
    // TODO(perf): Limit the query cursor to `from..to` once textobject queries support ranges.
    let Some(nodes) = textobject_query.capture_nodes(&capture_name, &root, text) else {
        return SmallVec::new();
    };

    nodes
        .filter_map(|node| {
            let start_byte = node.start_byte();
            let end_byte = node.end_byte();
            if start_byte > text.len() || end_byte > text.len() {
                return None;
            }

            let object_range = Range::new(
                text.byte_to_char_idx(start_byte),
                text.byte_to_char_idx(end_byte),
            )
            .with_direction(range.direction());

            range.contains_range(&object_range).then_some(object_range)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn comment_textobject_falls_back_from_injected_comment_language() {
        let loader = crate::loader::default_lang_loader(::loader::syntax::Resources::default());
        let doc = editor_core::Rope::from("fn main() {\n    // a comment\n    let x = 1;\n}\n");
        let syntax = Syntax::new(
            doc.slice(..),
            loader.language_for_name("rust").unwrap(),
            &loader,
        )
        .unwrap();
        assert_eq!(
            textobject_treesitter(
                doc.slice(..),
                Range::point(20),
                TextObject::Inside,
                "comment",
                &syntax,
                &loader,
                1
            ),
            Range::new(16, 28)
        );
    }

    use super::*;
    use editor_core::textobject::TextObject::*;
    use editor_core::Rope;
    #[test]
    fn test_textobject_surround() {
        // (text, [(cursor position, textobject, final range, surround char, count), ...])
        let tests = &[
            (
                "simple (single) surround pairs",
                vec![
                    (3, Inside, (3, 3), '(', 1),
                    (7, Inside, (8, 14), ')', 1),
                    (10, Inside, (8, 14), '(', 1),
                    (14, Inside, (8, 14), ')', 1),
                    (3, Around, (3, 3), '(', 1),
                    (7, Around, (7, 15), ')', 1),
                    (10, Around, (7, 15), '(', 1),
                    (14, Around, (7, 15), ')', 1),
                ],
            ),
            (
                "samexx 'single' surround pairs",
                vec![
                    (3, Inside, (3, 3), '\'', 1),
                    (7, Inside, (7, 7), '\'', 1),
                    (10, Inside, (8, 14), '\'', 1),
                    (14, Inside, (14, 14), '\'', 1),
                    (3, Around, (3, 3), '\'', 1),
                    (7, Around, (7, 7), '\'', 1),
                    (10, Around, (7, 15), '\'', 1),
                    (14, Around, (14, 14), '\'', 1),
                ],
            ),
            (
                "(nested (surround (pairs)) 3 levels)",
                vec![
                    (0, Inside, (1, 35), '(', 1),
                    (6, Inside, (1, 35), ')', 1),
                    (8, Inside, (9, 25), '(', 1),
                    (8, Inside, (9, 35), ')', 2),
                    (20, Inside, (9, 25), '(', 2),
                    (20, Inside, (1, 35), ')', 3),
                    (0, Around, (0, 36), '(', 1),
                    (6, Around, (0, 36), ')', 1),
                    (8, Around, (8, 26), '(', 1),
                    (8, Around, (8, 36), ')', 2),
                    (20, Around, (8, 26), '(', 2),
                    (20, Around, (0, 36), ')', 3),
                ],
            ),
            (
                "(mixed {surround [pair] same} line)",
                vec![
                    (2, Inside, (1, 34), '(', 1),
                    (9, Inside, (8, 28), '{', 1),
                    (18, Inside, (18, 22), '[', 1),
                    (2, Around, (0, 35), '(', 1),
                    (9, Around, (7, 29), '{', 1),
                    (18, Around, (17, 23), '[', 1),
                ],
            ),
            (
                "(stepped (surround) pairs (should) skip)",
                vec![(22, Inside, (1, 39), '(', 1), (22, Around, (0, 40), '(', 1)],
            ),
            (
                "[surround pairs{\non different]\nlines}",
                vec![
                    (7, Inside, (1, 29), '[', 1),
                    (15, Inside, (16, 36), '{', 1),
                    (7, Around, (0, 30), '[', 1),
                    (15, Around, (15, 37), '{', 1),
                ],
            ),
        ];

        for (sample, scenario) in tests {
            let doc = Rope::from(*sample);
            let slice = doc.slice(..);
            for &case in scenario {
                let (pos, objtype, expected_range, ch, count) = case;
                let result =
                    textobject_pair_surround(None, slice, Range::point(pos), objtype, ch, count);
                assert_eq!(
                    result,
                    expected_range.into(),
                    "\nCase failed: {:?} - {:?}",
                    sample,
                    case
                );
            }
        }
    }
}

#[cfg(test)]
mod collection_tests {
    use super::*;
    use editor_core::{movement::Direction, Rope};

    #[test]
    fn query_collections_preserve_unicode_direction_and_captures_at_eof() {
        let loader = syntax::Loader::new(
            toml::from_str(
                "[[language]]\nname = 'rust'\nscope = 'source.rust'\nfile-types = ['rs']",
            )
            .unwrap(),
            loader::syntax::Resources::default(),
        )
        .unwrap();
        let text = Rope::from_str("fn café() {}\nfn b() {}");
        let syntax = Syntax::new(
            text.slice(..),
            loader.language_for_name("rust").unwrap(),
            &loader,
        )
        .unwrap();
        let ranges = textobject_treesitter_ranges(
            text.slice(..),
            Range::new(text.len_chars(), 0),
            TextObject::Around,
            "function",
            Some(&syntax),
            &loader,
        );
        let fragments: Vec<_> = ranges
            .iter()
            .map(|range| range.fragment(text.slice(..)).into_owned())
            .collect();
        assert_eq!(fragments, ["fn café() {}", "fn b() {}"]);
        assert!(ranges
            .iter()
            .all(|range| range.direction() == Direction::Backward));
        let first = ranges[0].with_direction(Direction::Forward);
        let contained = textobject_treesitter_ranges(
            text.slice(..),
            first,
            TextObject::Around,
            "function",
            Some(&syntax),
            &loader,
        );
        assert_eq!(contained.as_slice(), &[first]);
        assert!(textobject_treesitter_ranges(
            text.slice(..),
            first,
            TextObject::Around,
            "function",
            None,
            &loader
        )
        .is_empty());
    }
}

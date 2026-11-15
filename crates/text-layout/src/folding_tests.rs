//! Folding must have one interpretation across formatting, hit testing, and motion.
use crate::{
    doc_formatter::{DocumentFormatter, TextFormat},
    movement::{move_vertically, move_vertically_visual},
    text_annotations::{Conceal, InlineAnnotation, LineAnnotation, Overlay, TextAnnotations},
    *,
};
use editor_core::{
    line_ending::line_end_char_index,
    movement::{Direction, Movement},
    Range, Rope,
};
use stdx::rope::RopeExt as _;

fn fold(text: editor_core::RopeSlice, start: usize, end: usize) -> Conceal {
    Conceal::new(line_end_char_index(&text, start)..line_end_char_index(&text, end))
}

#[test]
fn collapsed_labels_wrap_and_hit_test_each_grapheme_at_the_fold_anchor() {
    use crate::doc_formatter::GraphemeSource;

    for newline in ["\n", "\r\n"] {
        for width in [4, 8, 40] {
            let rope = Rope::from_str(&format!(
                "before{newline}if {{{newline}hidden{newline}}}; tail{newline}next"
            ));
            let text = rope.slice(..);
            let start = text.line_to_char_idx(1) + 4;
            let end = text.line_to_char_idx(3);
            let folds = [Conceal::new(start..end).with_text("界e\u{301} label 😀")];
            let mut annotations = TextAnnotations::default();
            annotations.set_conceals(&folds);
            let format = TextFormat {
                soft_wrap: true,
                viewport_width: width,
                max_wrap: 0,
                max_indent_retain: 0,
                ..Default::default()
            };
            let graphemes: Vec<_> =
                DocumentFormatter::new_at_prev_checkpoint(text, &format, &annotations, 0).collect();
            let labels: Vec<_> = graphemes
                .iter()
                .filter(|g| matches!(g.source, GraphemeSource::Conceal { .. }))
                .collect();
            assert_eq!(
                labels.iter().map(|g| g.raw.to_string()).collect::<String>(),
                "界e\u{301} label 😀"
            );
            assert_eq!(
                labels.iter().map(|g| g.doc_chars()).sum::<usize>(),
                end - start
            );
            for label in &labels {
                assert_eq!(label.char_idx, start);
                assert!(!label.source.is_eof());
                for column in label.visual_pos.col..label.visual_pos.col + label.width() {
                    assert!(
                        !hit_test(
                            text,
                            0,
                            label.visual_pos.row as isize,
                            column,
                            &format,
                            &annotations
                        )
                        .is_virtual
                    );
                    assert_eq!(
                        char_idx_at_visual_block_offset(
                            text,
                            0,
                            label.visual_pos.row,
                            column,
                            &format,
                            &annotations
                        ),
                        (start, label.visual_pos.row - labels[0].visual_pos.row)
                    );
                }
            }
            for hidden in start..end {
                assert_eq!(
                    visual_offset_from_block(text, 0, hidden, &format, &annotations).0,
                    labels[0].visual_pos
                );
                assert_eq!(
                    visual_offset_from_anchor(text, 0, hidden, &format, &annotations, usize::MAX)
                        .unwrap()
                        .0,
                    labels[0].visual_pos
                );
            }
            for suffix in graphemes.iter().filter(|g| g.char_idx >= end) {
                assert_eq!(
                    visual_offset_from_block(text, 0, suffix.char_idx, &format, &annotations).0,
                    suffix.visual_pos
                );
            }
            let from_inside =
                DocumentFormatter::new_at_prev_checkpoint(text, &format, &annotations, end - 1)
                    .collect::<Vec<_>>();
            assert_eq!(from_inside[0].char_idx, text.line_to_char_idx(1));
        }
    }
}

#[test]
fn wrapped_labels_scroll_both_directions_and_visual_motion_crosses_them_atomically() {
    let rope = Rope::from_str("before\nabHIDDENcd\nnext");
    let text = rope.slice(..);
    let start = text.line_to_char_idx(1) + 2;
    let end = start + 6;
    let folds = [Conceal::new(start..end).with_text("long label with words 😀 and more")];
    let mut annotations = TextAnnotations::default();
    annotations.set_conceals(&folds);
    for width in [4, 8] {
        let format = TextFormat {
            soft_wrap: true,
            viewport_width: width,
            max_wrap: 0,
            max_indent_retain: 0,
            ..Default::default()
        };
        let end_row = visual_offset_from_block(text, 0, end, &format, &annotations)
            .0
            .row;
        let mut anchor = 0;
        let mut offset = 0;
        for target_row in 1..=end_row + 1 {
            (anchor, offset) = char_idx_at_visual_offset(
                text,
                anchor,
                (offset + 1) as isize,
                0,
                &format,
                &annotations,
            );
            assert_eq!(
                visual_offset_from_block(text, 0, anchor, &format, &annotations)
                    .0
                    .row
                    + offset,
                target_row
            );
        }
        for target_row in (0..=end_row).rev() {
            (anchor, offset) = char_idx_at_visual_offset(
                text,
                anchor,
                offset as isize - 1,
                0,
                &format,
                &annotations,
            );
            assert_eq!(
                visual_offset_from_block(text, 0, anchor, &format, &annotations)
                    .0
                    .row
                    + offset,
                target_row
            );
        }
        for count in [1, 2] {
            let down = move_vertically_visual(
                text,
                Range::point(start),
                Direction::Forward,
                count,
                Movement::Move,
                &format,
                &mut annotations,
            );
            assert_eq!(down.cursor(text), end);
            let up = move_vertically_visual(
                text,
                down,
                Direction::Backward,
                count,
                Movement::Move,
                &format,
                &mut annotations,
            );
            assert_eq!(up.cursor(text), start);
            let up = move_vertically_visual(
                text,
                up,
                Direction::Backward,
                1,
                Movement::Move,
                &format,
                &mut annotations,
            );
            assert!(up.cursor(text) < start);
        }
    }
}

#[test]
fn virtual_rows_after_a_collapsed_label_still_reject_document_hits() {
    let rope = Rope::from_str("abHIDDENtail");
    let text = rope.slice(..);
    let folds = [Conceal::new(2..8).with_text("long long long label")];
    let inline = [InlineAnnotation::new(8, "\nvirtual\n")];
    let mut annotations = TextAnnotations::default();
    annotations.set_conceals(&folds);
    annotations.add_inline_annotations(&inline, None);
    let format = TextFormat {
        soft_wrap: true,
        viewport_width: 8,
        max_wrap: 0,
        max_indent_retain: 0,
        ..Default::default()
    };
    let virtual_row = DocumentFormatter::new_at_prev_checkpoint(text, &format, &annotations, 0)
        .find(|g| g.is_virtual() && g.raw.to_string() == "v")
        .unwrap()
        .visual_pos
        .row;
    assert!(hit_test(text, 0, virtual_row as isize, 0, &format, &annotations).is_virtual);
}

#[test]
fn collapsed_labels_cannot_insert_controls_or_display_lines() {
    let fold = Conceal::new(1..2).with_text("a\n\r\t\x1b\u{85}\u{2028}\u{2029}b");
    assert_eq!(fold.text(), "a       b");
    assert_eq!(Conceal::new(1..2).with_text("").text(), "…");
}

#[test]
fn folded_positions_round_trip_with_wrapping_unicode_and_line_endings() {
    for newline in ["\n", "\r\n"] {
        for soft_wrap in [false, true] {
            for ending in ["", newline] {
                let rope = Rope::from_str(&format!(
                    "é\t界e\u{301} header{newline}hidden{newline}}}{newline}next{ending}"
                ));
                let text = rope.slice(..);
                let folds = [fold(text, 0, 2)];
                let mut annotations = TextAnnotations::default();
                annotations.set_conceals(&folds);
                let format = TextFormat {
                    soft_wrap,
                    viewport_width: 8,
                    max_wrap: 0,
                    max_indent_retain: 0,
                    ..Default::default()
                };
                let graphemes: Vec<_> =
                    DocumentFormatter::new_at_prev_checkpoint(text, &format, &annotations, 0)
                        .collect();
                assert_eq!(
                    graphemes
                        .iter()
                        .filter(|g| !g.source.is_eof())
                        .map(|g| if g.raw == editor_core::graphemes::Grapheme::Newline {
                            "\n".to_owned()
                        } else {
                            g.raw.to_string()
                        })
                        .collect::<String>(),
                    "é   界e\u{301} header…\nnext".to_string()
                        + if ending.is_empty() { "" } else { "\n" }
                );
                for g in &graphemes {
                    let display = visual_offset_from_anchor(
                        text,
                        0,
                        g.char_idx,
                        &format,
                        &annotations,
                        usize::MAX,
                    )
                    .unwrap()
                    .0;
                    assert_eq!(display, g.visual_pos, "{g:?}");
                    assert_eq!(
                        char_idx_at_visual_offset(
                            text,
                            0,
                            display.row as isize,
                            display.col,
                            &format,
                            &annotations
                        ),
                        (g.char_idx, 0),
                        "{g:?}"
                    );
                }
                let next = graphemes
                    .iter()
                    .find(|g| g.char_idx == text.line_to_char_idx(3))
                    .unwrap();
                assert_eq!(next.line_idx, 3);
                for hidden in folds[0].range.start..=folds[0].range.end {
                    let formatter = DocumentFormatter::new_at_prev_checkpoint(
                        text,
                        &format,
                        &annotations,
                        hidden,
                    );
                    assert_eq!(formatter.next_char_pos(), 0);
                }
            }
        }
    }
}

#[test]
fn motions_count_fold_headers_and_preserve_columns_in_both_directions() {
    let rope = Rope::from_str("abcdefghijklm\nhidden\n}\nnext line\nlast\n}");
    let text = rope.slice(..);
    let folds = [fold(text, 0, 2), fold(text, 4, 5)];
    for (from_index, from) in [0, 3, 4].into_iter().enumerate() {
        for (to_index, to) in [0, 3, 4].into_iter().enumerate() {
            assert_eq!(
                crate::movement::document_line_distance(text, from, to, &folds),
                from_index.abs_diff(to_index),
            );
        }
    }
    for soft_wrap in [false, true] {
        let format = TextFormat {
            soft_wrap,
            viewport_width: 6,
            max_wrap: 0,
            max_indent_retain: 0,
            ..Default::default()
        };
        let mut annotations = TextAnnotations::default();
        annotations.set_conceals(&folds);
        let start = Range::point(2);
        let next = move_vertically(
            text,
            start,
            Direction::Forward,
            1,
            Movement::Move,
            &format,
            &mut annotations,
        );
        assert_eq!(next.cursor(text), text.line_to_char_idx(3) + 2);
        let previous = move_vertically(
            text,
            next,
            Direction::Backward,
            1,
            Movement::Move,
            &format,
            &mut annotations,
        );
        assert_eq!(previous.cursor(text), 2);
        let last = move_vertically(
            text,
            start,
            Direction::Forward,
            2,
            Movement::Move,
            &format,
            &mut annotations,
        );
        assert_eq!(last.cursor(text), text.line_to_char_idx(4) + 2);
        let back = move_vertically(
            text,
            last,
            Direction::Backward,
            2,
            Movement::Extend,
            &format,
            &mut annotations,
        );
        assert_eq!(back.cursor(text), 2);
        assert!(back.to() > last.cursor(text));
        if soft_wrap {
            let wrapped = move_vertically_visual(
                text,
                start,
                Direction::Forward,
                1,
                Movement::Move,
                &format,
                &mut annotations,
            );
            assert_eq!(wrapped.cursor(text), 8);
        }
        // A large column on the folded header clamps to its visible line ending.
        let from = Range::point(text.line_to_char_idx(3) + 8);
        let header = move_vertically(
            text,
            from,
            Direction::Backward,
            1,
            Movement::Move,
            &format,
            &mut annotations,
        );
        assert!(header.cursor(text) <= folds[0].range.end);
        assert!(
            header.cursor(text) <= folds[0].range.start
                || header.cursor(text) == folds[0].range.end
        );
    }
}

#[test]
fn annotations_inside_folds_are_skipped_and_following_annotations_survive() {
    use std::{cell::RefCell, rc::Rc};
    #[derive(Clone)]
    struct Anchors {
        next: usize,
        seen: Rc<RefCell<Vec<usize>>>,
    }
    impl LineAnnotation for Anchors {
        fn reset_pos(&mut self, pos: usize) -> usize {
            self.next = pos;
            pos
        }
        fn process_anchor(&mut self, g: &doc_formatter::FormattedGrapheme) -> usize {
            self.seen.borrow_mut().push(g.char_idx);
            self.next += g.doc_chars().max(1);
            self.next
        }
        fn insert_virtual_lines(
            &mut self,
            _: usize,
            _: DisplayPosition,
            _: usize,
        ) -> DisplayPosition {
            DisplayPosition::default()
        }
    }
    let rope = Rope::from_str("a\nhidden\n}\nz");
    let text = rope.slice(..);
    let folds = [fold(text, 0, 2)];
    let after = text.line_to_char_idx(3);
    let inline = [
        InlineAnnotation::new(3, "HIDDEN"),
        InlineAnnotation::new(after, "hint"),
    ];
    let overlays = [Overlay::new(3, "X"), Overlay::new(after, "Z")];
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut annotations = TextAnnotations::default();
    annotations.set_conceals(&folds);
    annotations
        .add_inline_annotations(&inline, None)
        .add_overlay(&overlays, None)
        .add_line_annotation(Anchors {
            next: 0,
            seen: seen.clone(),
        });
    let format = TextFormat::default();
    let rendered: String =
        DocumentFormatter::new_at_prev_checkpoint(text, &format, &annotations, 0)
            .filter(|g| !g.source.is_eof())
            .map(|g| {
                if g.raw == editor_core::graphemes::Grapheme::Newline {
                    "\n".to_owned()
                } else {
                    g.raw.to_string()
                }
            })
            .collect();
    assert_eq!(rendered, "a…\nhintZ");
    assert!(seen
        .borrow()
        .iter()
        .all(|p| *p <= folds[0].range.start || *p >= folds[0].range.end));
    assert!(seen.borrow().contains(&after));
}

#[test]
fn large_folds_skip_the_rope_instead_of_traversing_hidden_graphemes() {
    let rope = Rope::from_str(&format!("header\n{}end", "界\n".repeat(100_000)));
    let text = rope.slice(..);
    let folds = [fold(text, 0, 100_000)];
    let mut annotations = TextAnnotations::default();
    annotations.set_conceals(&folds);
    let format = TextFormat::default();
    let graphemes: Vec<_> =
        DocumentFormatter::new_at_prev_checkpoint(text, &format, &annotations, 0).collect();
    assert!(graphemes.len() < 20);
    assert_eq!(
        graphemes
            .iter()
            .find(|g| g.char_idx == text.line_to_char_idx(100_001))
            .unwrap()
            .line_idx,
        100_001
    );
}

#[test]
fn motion_from_a_folded_line_ending_uses_the_header_line_and_clamps_at_eof() {
    let rope = Rope::from_str("before\nheader\nhidden\n}");
    let text = rope.slice(..);
    let folds = [fold(text, 1, 3)];
    let mut annotations = TextAnnotations::default();
    annotations.set_conceals(&folds);
    let format = TextFormat::default();
    let end = Range::point(folds[0].range.end);
    let up = move_vertically(
        text,
        end,
        Direction::Backward,
        1,
        Movement::Move,
        &format,
        &mut annotations,
    );
    assert_eq!(text.char_to_line_idx(up.cursor(text)), 0);
    let header = Range::point(text.line_to_char_idx(1));
    for count in [1, 2, usize::MAX] {
        let down = move_vertically(
            text,
            header,
            Direction::Forward,
            count,
            Movement::Move,
            &format,
            &mut annotations,
        );
        assert_eq!(down.cursor(text), header.cursor(text));
    }
}

#[test]
fn preserved_suffixes_round_trip_and_share_the_headers_wrapped_rows() {
    for newline in ["\n", "\r\n"] {
        for width in [4, 8, 40] {
            for ending in ["", "\nnext"] {
                // Two disjoint folds share a source line: the visible suffix of
                // the first is also the header of the second.
                let rope = Rope::from_str(&format!("if {{{newline}hidden{newline}}} else {{{newline}hidden{newline}}}; 界e\u{301}{ending}"));
                let text = rope.slice(..);
                let folds = [
                    Conceal::new(line_end_char_index(&text, 0)..text.line_to_char_idx(2)),
                    Conceal::new(line_end_char_index(&text, 2)..text.line_to_char_idx(4)),
                ];
                let mut annotations = TextAnnotations::default();
                annotations.set_conceals(&folds);
                let format = TextFormat {
                    soft_wrap: true,
                    viewport_width: width,
                    max_wrap: 0,
                    max_indent_retain: 0,
                    ..Default::default()
                };
                let graphemes: Vec<_> =
                    DocumentFormatter::new_at_prev_checkpoint(text, &format, &annotations, 0)
                        .collect();
                let rendered: String = graphemes
                    .iter()
                    .filter(|g| !g.source.is_eof())
                    .map(|g| {
                        if g.raw == editor_core::graphemes::Grapheme::Newline {
                            "\n".to_owned()
                        } else {
                            g.raw.to_string()
                        }
                    })
                    .collect();
                assert_eq!(rendered, format!("if {{…}} else {{…}}; 界e\u{301}{ending}"));
                for g in graphemes {
                    let display = visual_offset_from_anchor(
                        text,
                        0,
                        g.char_idx,
                        &format,
                        &annotations,
                        usize::MAX,
                    )
                    .unwrap()
                    .0;
                    assert_eq!(display, g.visual_pos, "{g:?}");
                    assert_eq!(
                        char_idx_at_visual_offset(
                            text,
                            0,
                            display.row as isize,
                            display.col,
                            &format,
                            &annotations
                        ),
                        (g.char_idx, 0),
                        "{g:?}"
                    );
                    let source_line = text.char_to_line_idx(g.char_idx);
                    assert_eq!(
                        g.block_line_idx,
                        if source_line <= 4 { 0 } else { source_line }
                    );
                    assert_eq!(g.line_idx, source_line);
                    if source_line <= 4 {
                        assert_eq!(
                            DocumentFormatter::new_at_prev_checkpoint(
                                text,
                                &format,
                                &annotations,
                                g.char_idx
                            )
                            .next_char_pos(),
                            0
                        );
                    }
                }
                // Document-line motion starts from the displayed header even
                // when the cursor is on a closer from its last source line.
                let suffix = Range::point(folds[1].range.end + 1);
                let down = move_vertically(
                    text,
                    suffix,
                    Direction::Forward,
                    1,
                    Movement::Move,
                    &format,
                    &mut annotations,
                );
                if ending.is_empty() {
                    assert_eq!(down.cursor(text), suffix.cursor(text));
                } else {
                    assert_eq!(text.char_to_line_idx(down.cursor(text)), 5);
                }
            }
        }
    }
}

//! Ordinary text motions, independent of viewport layout and terminal input.

use editor_core::{
    graphemes,
    line_ending::line_end_char_index,
    movement::{self, Direction, Movement},
};
use stdx::rope::{RopeExt as _, RopeSliceExt as _};

use crate::{Document, ViewId};

#[derive(Clone, Copy, Debug)]
pub enum LineBoundary {
    Start,
    End,
    Ending,
    FirstNonWhitespace,
}

pub use editor_core::movement::WordMotionTarget;

#[derive(Clone, Copy, Debug)]
pub enum TextMotion {
    Character(Direction),
    LineBoundary(LineBoundary),
    Word(WordMotionTarget),
    Paragraph(Direction),
}

/// Move or extend every selection, then publish normalized selections once.
/// Word movement retains the word-selection semantics of the underlying motion;
/// extension keeps the original anchor. A zero count leaves selections untouched.
pub fn move_text(
    doc: &mut Document,
    view: ViewId,
    motion: TextMotion,
    count: usize,
    behaviour: Movement,
) {
    if count == 0 {
        return;
    }
    let text = doc.text().slice(..);
    let selection = doc.selection(view).clone().transform(|range| match motion {
        TextMotion::Character(direction) => {
            movement::move_horizontally(text, range, direction, count, behaviour)
        }
        TextMotion::Paragraph(Direction::Forward) => {
            movement::move_next_paragraph(text, range, count, behaviour)
        }
        TextMotion::Paragraph(Direction::Backward) => {
            movement::move_prev_paragraph(text, range, count, behaviour)
        }
        TextMotion::Word(motion) => {
            let word = movement::move_word(text, range, count, motion);
            if behaviour == Movement::Extend {
                range.put_cursor(text, word.cursor(text), true)
            } else {
                word
            }
        }
        TextMotion::LineBoundary(boundary) => {
            let line = range.cursor_line(text);
            let start = text.line_to_char_idx(line);
            let position = match boundary {
                LineBoundary::Start => start,
                LineBoundary::End => {
                    graphemes::prev_grapheme_boundary(text, line_end_char_index(&text, line))
                        .max(start)
                }
                LineBoundary::Ending => line_end_char_index(&text, line),
                LineBoundary::FirstNonWhitespace => {
                    let Some(offset) = text
                        .line(line, stdx::rope::LINE_TYPE)
                        .first_non_whitespace_char()
                    else {
                        return range;
                    };
                    start + offset
                }
            };
            range.put_cursor(text, position, behaviour == Movement::Extend)
        }
    });
    doc.set_selection(view, selection);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing::tests::document_and_view;
    use editor_core::{Range, Selection};

    #[tokio::test]
    async fn line_boundaries_respect_combining_graphemes_and_crlf() {
        for ending in ["\n", "\r\n"] {
            let (mut doc, view) = document_and_view(&format!("  e\u{301}界#[|x]#{ending}"));
            let original = doc.selection(view.id).clone();
            for (boundary, expected) in [
                (LineBoundary::Start, 0),
                (LineBoundary::End, 5),
                (LineBoundary::Ending, 6),
                (LineBoundary::FirstNonWhitespace, 2),
            ] {
                doc.set_selection(view.id, original.clone());
                move_text(
                    &mut doc,
                    view.id,
                    TextMotion::LineBoundary(boundary),
                    1,
                    Movement::Move,
                );
                assert_eq!(
                    doc.selection(view.id)
                        .primary()
                        .cursor(doc.text().slice(..)),
                    expected
                );
            }
            assert_eq!(doc.version(), 0);
        }
    }

    #[tokio::test]
    async fn extending_line_motion_preserves_reversed_anchors_and_primary_selection() {
        let (mut doc, view) = document_and_view("#[|o]#ne two\nred blue\n");
        doc.set_selection(
            view.id,
            Selection::new(vec![Range::new(3, 1), Range::new(11, 9)].into(), 1),
        );
        move_text(
            &mut doc,
            view.id,
            TextMotion::LineBoundary(LineBoundary::Start),
            1,
            Movement::Extend,
        );
        assert_eq!(
            doc.selection(view.id),
            &Selection::new(vec![Range::new(3, 0), Range::new(11, 8)].into(), 1)
        );
    }

    #[tokio::test]
    async fn counted_word_motion_distinguishes_movement_from_extension() {
        let (mut doc, view) = document_and_view("#[|o]#ne two three\n");
        let original = doc.selection(view.id).clone();
        let motion = TextMotion::Word(WordMotionTarget::NextWordStart);
        move_text(&mut doc, view.id, motion, 2, Movement::Move);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            7
        );
        doc.set_selection(view.id, original);
        move_text(&mut doc, view.id, motion, 2, Movement::Extend);
        assert_eq!(doc.selection(view.id), &Selection::single(0, 8));
    }

    #[tokio::test]
    async fn whitespace_only_lines_and_zero_counts_preserve_selections() {
        let (mut doc, view) = document_and_view(" #[| ]# \r\n");
        let original = doc.selection(view.id).clone();
        move_text(
            &mut doc,
            view.id,
            TextMotion::LineBoundary(LineBoundary::FirstNonWhitespace),
            1,
            Movement::Move,
        );
        assert_eq!(doc.selection(view.id), &original);
        move_text(
            &mut doc,
            view.id,
            TextMotion::Character(Direction::Forward),
            0,
            Movement::Extend,
        );
        assert_eq!(doc.selection(view.id), &original);
    }
}

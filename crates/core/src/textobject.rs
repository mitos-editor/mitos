use std::fmt::Display;
use stdx::rope::RopeExt as _;

use ropey::RopeSlice;

use crate::chars::{categorize_char, char_is_whitespace, CharCategory};
use crate::graphemes::prev_grapheme_boundary;
use crate::line_ending::rope_is_line_ending;
use crate::movement::Direction;
use crate::Range;

fn find_word_boundary(slice: RopeSlice, mut pos: usize, direction: Direction, long: bool) -> usize {
    use CharCategory::{Eol, Whitespace};

    let iter = match direction {
        Direction::Forward => slice.chars_at_char(pos),
        Direction::Backward => slice.chars_at_char(pos).reversed(),
    };

    let mut prev_category = match direction {
        Direction::Forward if pos == 0 => Whitespace,
        Direction::Forward => categorize_char(slice.char_at(pos - 1)),
        Direction::Backward if pos == slice.len_chars() => Whitespace,
        Direction::Backward => categorize_char(slice.char_at(pos)),
    };

    for ch in iter {
        match categorize_char(ch) {
            Eol | Whitespace => return pos,
            category => {
                if !long && category != prev_category && pos != 0 && pos != slice.len_chars() {
                    return pos;
                } else {
                    match direction {
                        Direction::Forward => pos += 1,
                        Direction::Backward => pos = pos.saturating_sub(1),
                    }
                    prev_category = category;
                }
            }
        }
    }

    pos
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TextObject {
    Around,
    Inside,
    /// Used for moving between objects.
    Movement,
}

impl Display for TextObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Around => "around",
            Self::Inside => "inside",
            Self::Movement => "movement",
        })
    }
}

// count doesn't do anything yet
pub fn textobject_word(
    slice: RopeSlice,
    range: Range,
    textobject: TextObject,
    _count: usize,
    long: bool,
) -> Range {
    let pos = range.cursor(slice);

    let word_start = find_word_boundary(slice, pos, Direction::Backward, long);
    let word_end = match slice.get_char_at(pos).map(categorize_char) {
        None | Some(CharCategory::Whitespace | CharCategory::Eol) => pos,
        _ => find_word_boundary(slice, pos + 1, Direction::Forward, long),
    };

    // Special case.
    if word_start == word_end {
        return Range::new(word_start, word_end);
    }

    match textobject {
        TextObject::Inside => Range::new(word_start, word_end),
        TextObject::Around => {
            let whitespace_count_right = slice
                .chars_at_char(word_end)
                .take_while(|c| char_is_whitespace(*c))
                .count();

            if whitespace_count_right > 0 {
                Range::new(word_start, word_end + whitespace_count_right)
            } else {
                let whitespace_count_left = {
                    let iter = slice.chars_at_char(word_start).reversed();
                    iter.take_while(|c| char_is_whitespace(*c)).count()
                };
                Range::new(word_start - whitespace_count_left, word_end)
            }
        }
        TextObject::Movement => unreachable!(),
    }
}

pub fn textobject_paragraph(
    slice: RopeSlice,
    range: Range,
    textobject: TextObject,
    count: usize,
) -> Range {
    let mut line = range.cursor_line(slice);
    let prev_line_empty =
        rope_is_line_ending(slice.line(line.saturating_sub(1), stdx::rope::LINE_TYPE));
    let curr_line_empty = rope_is_line_ending(slice.line(line, stdx::rope::LINE_TYPE));
    let next_line_empty = line + 1 >= slice.len_lines(stdx::rope::LINE_TYPE)
        || rope_is_line_ending(slice.line(line + 1, stdx::rope::LINE_TYPE));
    let last_char =
        prev_grapheme_boundary(slice, slice.line_to_char_idx(line + 1)) == range.cursor(slice);
    let prev_empty_to_line = prev_line_empty && !curr_line_empty;
    let curr_empty_to_line = curr_line_empty && !next_line_empty;

    // skip character before paragraph boundary
    let mut line_back = line; // line but backwards
    if prev_empty_to_line || curr_empty_to_line {
        line_back += 1;
    }
    // do not include current paragraph on paragraph end (include next)
    if !(curr_empty_to_line && last_char) {
        let lines = slice.lines_at(line_back, stdx::rope::LINE_TYPE).reversed();
        let mut lines = lines.map(rope_is_line_ending).peekable();
        while lines.next_if(|&e| e).is_some() {
            line_back -= 1;
        }
        while lines.next_if(|&e| !e).is_some() {
            line_back -= 1;
        }
    }

    // skip character after paragraph boundary
    if curr_empty_to_line && last_char {
        line += 1;
    }
    let mut lines = slice
        .lines_at(line, stdx::rope::LINE_TYPE)
        .map(rope_is_line_ending)
        .peekable();
    let mut count_done = 0; // count how many non-whitespace paragraphs done
    for _ in 0..count {
        let mut done = false;
        while lines.next_if(|&e| !e).is_some() {
            line += 1;
            done = true;
        }
        while lines.next_if(|&e| e).is_some() {
            line += 1;
        }
        count_done += done as usize;
    }

    // search one paragraph backwards for last paragraph
    // makes `map` at the end of the paragraph with trailing newlines useful
    let last_paragraph = count_done != count && lines.peek().is_none();
    if last_paragraph {
        let lines = slice.lines_at(line_back, stdx::rope::LINE_TYPE).reversed();
        let mut lines = lines.map(rope_is_line_ending).peekable();
        while lines.next_if(|&e| e).is_some() {
            line_back -= 1;
        }
        while lines.next_if(|&e| !e).is_some() {
            line_back -= 1;
        }
    }

    // handle last whitespaces part separately depending on textobject
    match textobject {
        TextObject::Around => {}
        TextObject::Inside => {
            // remove last whitespace paragraph
            let lines = slice.lines_at(line, stdx::rope::LINE_TYPE).reversed();
            let mut lines = lines.map(rope_is_line_ending).peekable();
            while lines.next_if(|&e| e).is_some() {
                line -= 1;
            }
        }
        TextObject::Movement => unreachable!(),
    }

    let anchor = slice.line_to_char_idx(line_back);
    let head = slice.line_to_char_idx(line);
    Range::new(anchor, head)
}

/// Collect word objects fully contained in `range`, preserving selection direction.
pub fn word_ranges(
    text: RopeSlice,
    range: Range,
    object: TextObject,
    long: bool,
) -> crate::SmallVec<[Range; 1]> {
    contained_ranges(text, range, |cursor| {
        textobject_word(text, cursor, object, 1, long)
    })
}

/// Collect paragraph objects fully contained in `range`, preserving selection direction.
pub fn paragraph_ranges(
    text: RopeSlice,
    range: Range,
    object: TextObject,
) -> crate::SmallVec<[Range; 1]> {
    contained_ranges(text, range, |cursor| {
        textobject_paragraph(text, cursor, object, 1)
    })
}

fn contained_ranges(
    text: RopeSlice,
    range: Range,
    locate: impl Fn(Range) -> Range,
) -> crate::SmallVec<[Range; 1]> {
    let mut ranges = crate::SmallVec::new();
    let mut pos = range.from();
    while pos < range.to() {
        let object = locate(Range::point(pos)).with_direction(range.direction());
        if !object.is_empty() && object.to() > pos && range.contains_range(&object) {
            pos = object.to();
            ranges.push(object);
        } else {
            pos = crate::graphemes::next_grapheme_boundary(text, pos);
        }
    }
    ranges
}

#[cfg(test)]
mod test {
    use super::TextObject::*;
    use super::*;

    use crate::Range;
    use ropey::Rope;

    #[test]
    fn test_textobject_word() {
        // (text, [(char position, textobject, final range), ...])
        let tests = &[
            (
                "cursor at beginning of doc",
                vec![(0, Inside, (0, 6)), (0, Around, (0, 7))],
            ),
            (
                "cursor at middle of word",
                vec![
                    (13, Inside, (10, 16)),
                    (10, Inside, (10, 16)),
                    (15, Inside, (10, 16)),
                    (13, Around, (10, 17)),
                    (10, Around, (10, 17)),
                    (15, Around, (10, 17)),
                ],
            ),
            (
                "cursor between word whitespace",
                vec![(6, Inside, (6, 6)), (6, Around, (6, 6))],
            ),
            (
                "cursor on word before newline\n",
                vec![
                    (22, Inside, (22, 29)),
                    (28, Inside, (22, 29)),
                    (25, Inside, (22, 29)),
                    (22, Around, (21, 29)),
                    (28, Around, (21, 29)),
                    (25, Around, (21, 29)),
                ],
            ),
            (
                "cursor on newline\nnext line",
                vec![(17, Inside, (17, 17)), (17, Around, (17, 17))],
            ),
            (
                "cursor on word after newline\nnext line",
                vec![
                    (29, Inside, (29, 33)),
                    (30, Inside, (29, 33)),
                    (32, Inside, (29, 33)),
                    (29, Around, (29, 34)),
                    (30, Around, (29, 34)),
                    (32, Around, (29, 34)),
                ],
            ),
            (
                "cursor on #$%:;* punctuation",
                vec![
                    (13, Inside, (10, 16)),
                    (10, Inside, (10, 16)),
                    (15, Inside, (10, 16)),
                    (13, Around, (10, 17)),
                    (10, Around, (10, 17)),
                    (15, Around, (10, 17)),
                ],
            ),
            (
                "cursor on punc%^#$:;.tuation",
                vec![
                    (14, Inside, (14, 21)),
                    (20, Inside, (14, 21)),
                    (17, Inside, (14, 21)),
                    (14, Around, (14, 21)),
                    (20, Around, (14, 21)),
                    (17, Around, (14, 21)),
                ],
            ),
            (
                "cursor in   extra whitespace",
                vec![
                    (9, Inside, (9, 9)),
                    (10, Inside, (10, 10)),
                    (11, Inside, (11, 11)),
                    (9, Around, (9, 9)),
                    (10, Around, (10, 10)),
                    (11, Around, (11, 11)),
                ],
            ),
            (
                "cursor on word   with extra whitespace",
                vec![(11, Inside, (10, 14)), (11, Around, (10, 17))],
            ),
            (
                "cursor at end with extra   whitespace",
                vec![(28, Inside, (27, 37)), (28, Around, (24, 37))],
            ),
            (
                "cursor at end of doc",
                vec![(19, Inside, (17, 20)), (19, Around, (16, 20))],
            ),
        ];

        for (sample, scenario) in tests {
            let doc = Rope::from(*sample);
            let slice = doc.slice(..);
            for &case in scenario {
                let (pos, objtype, expected_range) = case;
                // cursor is a single width selection
                let range = Range::new(pos, pos + 1);
                let result = textobject_word(slice, range, objtype, 1, false);
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

    #[test]
    fn test_textobject_paragraph_inside_single() {
        let tests = [
            ("#[|]#", "#[|]#"),
            ("firs#[t|]#\n\nparagraph\n\n", "#[first\n|]#\nparagraph\n\n"),
            (
                "second\n\npa#[r|]#agraph\n\n",
                "second\n\n#[paragraph\n|]#\n",
            ),
            ("#[f|]#irst char\n\n", "#[first char\n|]#\n"),
            ("last char\n#[\n|]#", "#[last char\n|]#\n"),
            (
                "empty to line\n#[\n|]#paragraph boundary\n\n",
                "empty to line\n\n#[paragraph boundary\n|]#\n",
            ),
            (
                "line to empty\n\n#[p|]#aragraph boundary\n\n",
                "line to empty\n\n#[paragraph boundary\n|]#\n",
            ),
        ];

        for (before, expected) in tests {
            let (s, selection) = crate::test::print(before);
            let text = Rope::from(s.as_str());
            let selection = selection
                .transform(|r| textobject_paragraph(text.slice(..), r, TextObject::Inside, 1));
            let actual = crate::test::plain(s.as_ref(), &selection);
            assert_eq!(actual, expected, "\nbefore: `{:?}`", before);
        }
    }

    #[test]
    fn test_textobject_paragraph_inside_double() {
        let tests = [
            (
                "last two\n\n#[p|]#aragraph\n\nwithout whitespaces\n\n",
                "last two\n\n#[paragraph\n\nwithout whitespaces\n|]#\n",
            ),
            (
                "last two\n#[\n|]#paragraph\n\nwithout whitespaces\n\n",
                "last two\n\n#[paragraph\n\nwithout whitespaces\n|]#\n",
            ),
        ];

        for (before, expected) in tests {
            let (s, selection) = crate::test::print(before);
            let text = Rope::from(s.as_str());
            let selection = selection
                .transform(|r| textobject_paragraph(text.slice(..), r, TextObject::Inside, 2));
            let actual = crate::test::plain(s.as_ref(), &selection);
            assert_eq!(actual, expected, "\nbefore: `{:?}`", before);
        }
    }

    #[test]
    fn test_textobject_paragraph_around_single() {
        let tests = [
            ("#[|]#", "#[|]#"),
            ("firs#[t|]#\n\nparagraph\n\n", "#[first\n\n|]#paragraph\n\n"),
            (
                "second\n\npa#[r|]#agraph\n\n",
                "second\n\n#[paragraph\n\n|]#",
            ),
            ("#[f|]#irst char\n\n", "#[first char\n\n|]#"),
            ("last char\n#[\n|]#", "#[last char\n\n|]#"),
            (
                "empty to line\n#[\n|]#paragraph boundary\n\n",
                "empty to line\n\n#[paragraph boundary\n\n|]#",
            ),
            (
                "line to empty\n\n#[p|]#aragraph boundary\n\n",
                "line to empty\n\n#[paragraph boundary\n\n|]#",
            ),
        ];

        for (before, expected) in tests {
            let (s, selection) = crate::test::print(before);
            let text = Rope::from(s.as_str());
            let selection = selection
                .transform(|r| textobject_paragraph(text.slice(..), r, TextObject::Around, 1));
            let actual = crate::test::plain(s.as_ref(), &selection);
            assert_eq!(actual, expected, "\nbefore: `{:?}`", before);
        }
    }
}

#[cfg(test)]
mod collection_tests {
    use super::*;
    use crate::Rope;

    #[test]
    fn word_collections_preserve_direction_and_exclude_partial_objects() {
        let text = Rope::from_str("é foo-bar");
        let range = Range::new(text.len_chars(), 0);
        let words = word_ranges(text.slice(..), range, TextObject::Inside, false);
        let fragments: Vec<_> = words
            .iter()
            .map(|range| range.fragment(text.slice(..)).into_owned())
            .collect();
        assert_eq!(fragments, ["é", "foo", "-", "bar"]);
        assert!(words
            .iter()
            .all(|range| range.direction() == Direction::Backward));
        let long = word_ranges(text.slice(..), range, TextObject::Inside, true);
        let fragments: Vec<_> = long
            .iter()
            .map(|range| range.fragment(text.slice(..)).into_owned())
            .collect();
        assert_eq!(fragments, ["é", "foo-bar"]);
        let partial = word_ranges(text.slice(..), Range::new(3, 8), TextObject::Inside, false);
        assert_eq!(partial.as_slice(), &[Range::new(5, 6)]);
        assert!(word_ranges(text.slice(..), Range::point(0), TextObject::Inside, false).is_empty());
    }

    #[test]
    fn paragraph_collections_cover_contained_paragraphs_without_repeating_the_last() {
        let text = Rope::from_str("one\n\nmiddle\n\nlast\n\n");
        let ranges = paragraph_ranges(
            text.slice(..),
            Range::new(0, text.len_chars()),
            TextObject::Inside,
        );
        let fragments: Vec<_> = ranges
            .iter()
            .map(|range| range.fragment(text.slice(..)).trim().to_owned())
            .collect();
        assert_eq!(fragments, ["one", "middle", "last"]);
    }
}

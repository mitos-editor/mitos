use ropey::RopeSlice;
use stdx::rope::RopeExt as _;

const MAX_PLAINTEXT_SCAN: usize = 10000;

pub const BRACKETS: [(char, char); 9] = [
    ('(', ')'),
    ('{', '}'),
    ('[', ']'),
    ('<', '>'),
    ('‘', '’'),
    ('“', '”'),
    ('«', '»'),
    ('「', '」'),
    ('（', '）'),
];

// The difference between BRACKETS and PAIRS is that we can find matching
// BRACKETS in a plain text file, but we can't do the same for PAIRs.
// PAIRS also contains all BRACKETS.
pub const PAIRS: [(char, char); BRACKETS.len() + 4] = {
    let mut pairs = [(' ', ' '); BRACKETS.len() + 4];
    let mut idx = 0;
    while idx < BRACKETS.len() {
        pairs[idx] = BRACKETS[idx];
        idx += 1;
    }
    pairs[idx] = ('"', '"');
    pairs[idx + 1] = ('\'', '\'');
    pairs[idx + 2] = ('`', '`');
    // Rust closure parameters use `|...|` delimiters. Tree-sitter exposes
    // them as a `closure_parameters` node whose first and last children are
    // both `|`, so the tree-sitter path in `find_pair` matches them
    // correctly without any false positives for bitwise-OR operators.
    pairs[idx + 3] = ('|', '|');
    pairs
};

/// Returns the position of the matching bracket under cursor.
/// This function works on plain text and ignores tree-sitter grammar.
/// The search is limited to `MAX_PLAINTEXT_SCAN` characters
///
/// If the cursor is on the opening bracket, the position of
/// the closing bracket is returned. If the cursor on the closing
/// bracket, the position of the opening bracket is returned.
///
/// If the cursor is not on a bracket, `None` is returned.
///
/// If no matching bracket is found, `None` is returned.
#[must_use]
pub fn find_matching_bracket_plaintext(doc: RopeSlice, cursor_pos: usize) -> Option<usize> {
    let bracket = doc.get_char_at(cursor_pos)?;
    let matching_bracket = {
        let pair = get_pair(bracket);
        if pair.0 == bracket {
            pair.1
        } else {
            pair.0
        }
    };
    // Don't do anything when the cursor is not on top of a bracket.
    if !is_valid_bracket(bracket) {
        return None;
    }

    // Determine the direction of the matching.
    let is_fwd = is_open_bracket(bracket);
    let chars_iter = if is_fwd {
        doc.chars_at_char(cursor_pos + 1)
    } else {
        doc.chars_at_char(cursor_pos).reversed()
    };

    let mut open_cnt = 1;

    for (i, candidate) in chars_iter.take(MAX_PLAINTEXT_SCAN).enumerate() {
        if candidate == bracket {
            open_cnt += 1;
        } else if candidate == matching_bracket {
            // Return when all pending brackets have been closed.
            if open_cnt == 1 {
                return Some(if is_fwd {
                    cursor_pos + i + 1
                } else {
                    cursor_pos - i - 1
                });
            }
            open_cnt -= 1;
        }
    }

    None
}

/// Returns the open and closing chars pair. If not found in
/// [`BRACKETS`] returns (ch, ch).
///
/// ```
/// use editor_core::match_brackets::get_pair;
///
/// assert_eq!(get_pair('['), ('[', ']'));
/// assert_eq!(get_pair('}'), ('{', '}'));
/// assert_eq!(get_pair('"'), ('"', '"'));
/// ```
pub fn get_pair(ch: char) -> (char, char) {
    PAIRS
        .iter()
        .find(|(open, close)| *open == ch || *close == ch)
        .copied()
        .unwrap_or((ch, ch))
}

pub fn is_open_bracket(ch: char) -> bool {
    BRACKETS.iter().any(|(l, _)| *l == ch)
}

pub fn is_close_bracket(ch: char) -> bool {
    BRACKETS.iter().any(|(_, r)| *r == ch)
}

pub fn is_valid_bracket(ch: char) -> bool {
    BRACKETS.iter().any(|(l, r)| *l == ch || *r == ch)
}

pub fn is_open_pair(ch: char) -> bool {
    PAIRS.iter().any(|(l, _)| *l == ch)
}

pub fn is_close_pair(ch: char) -> bool {
    PAIRS.iter().any(|(_, r)| *r == ch)
}

pub fn is_valid_pair(ch: char) -> bool {
    PAIRS.iter().any(|(l, r)| *l == ch || *r == ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_matching_bracket_empty_file() {
        let actual = find_matching_bracket_plaintext("".into(), 0);
        assert_eq!(actual, None);
    }

    #[test]
    fn test_find_matching_bracket_current_line_plaintext() {
        let assert = |input: &str, pos, expected| {
            let input = RopeSlice::from(input);
            let actual = find_matching_bracket_plaintext(input, pos);
            assert_eq!(expected, actual.unwrap());

            let actual = find_matching_bracket_plaintext(input, expected);
            assert_eq!(pos, actual.unwrap(), "expected symmetrical behaviour");
        };

        assert("(hello)", 0, 6);
        assert("((hello))", 0, 8);
        assert("((hello))", 1, 7);
        assert("(((hello)))", 2, 8);

        assert("key: ${value}", 6, 12);
        assert("key: ${value} # (some comment)", 16, 29);

        assert("(paren (paren {bracket}))", 0, 24);
        assert("(paren (paren {bracket}))", 7, 23);
        assert("(paren (paren {bracket}))", 14, 22);

        assert("(prev line\n ) (middle) ( \n next line)", 0, 12);
        assert("(prev line\n ) (middle) ( \n next line)", 14, 21);
        assert("(prev line\n ) (middle) ( \n next line)", 23, 36);
    }
}

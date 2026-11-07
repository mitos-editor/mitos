use crate::{Rope, RopeSlice};
use std::borrow::Cow;
use stdx::rope::RopeExt as _;

/// Normalize CRLF, bare CR, and LF sequences to the requested line ending.
/// Other Unicode line separators and text are left unchanged.
pub fn normalize_line_endings(text: &str, line_ending: LineEnding) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let mut endings = memchr::memchr2_iter(b'\r', b'\n', bytes).peekable();
    if endings.peek().is_none() {
        return Cow::Borrowed(text);
    }

    let mut normalized = String::with_capacity(text.len());
    let mut copied = 0;
    for index in endings {
        // A CRLF pair is one line ending; its LF was consumed with the CR.
        if index < copied {
            continue;
        }
        // ASCII CR and LF bytes are always UTF-8 boundaries.
        normalized.push_str(&text[copied..index]);
        normalized.push_str(line_ending.as_str());
        copied = index + 1;
        if bytes[index] == b'\r' && bytes.get(copied) == Some(&b'\n') {
            copied += 1;
        }
    }
    normalized.push_str(&text[copied..]);
    Cow::Owned(normalized)
}

#[cfg(target_os = "windows")]
pub const NATIVE_LINE_ENDING: LineEnding = LineEnding::Crlf;
#[cfg(not(target_os = "windows"))]
pub const NATIVE_LINE_ENDING: LineEnding = LineEnding::LF;

/// Represents one of the valid Unicode line endings.
#[derive(PartialEq, Eq, Copy, Clone, Debug)]
pub enum LineEnding {
    Crlf, // CarriageReturn followed by LineFeed
    LF,   // U+000A -- LineFeed
    #[cfg(feature = "unicode-lines")]
    VT, // U+000B -- VerticalTab
    #[cfg(feature = "unicode-lines")]
    FF, // U+000C -- FormFeed
    #[cfg(feature = "unicode-lines")]
    CR, // U+000D -- CarriageReturn
    #[cfg(feature = "unicode-lines")]
    Nel, // U+0085 -- NextLine
    #[cfg(feature = "unicode-lines")]
    LS, // U+2028 -- Line Separator
    #[cfg(feature = "unicode-lines")]
    PS, // U+2029 -- ParagraphSeparator
}

impl LineEnding {
    #[inline]
    pub const fn len_chars(&self) -> usize {
        match self {
            Self::Crlf => 2,
            _ => 1,
        }
    }

    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Crlf => "\u{000D}\u{000A}",
            Self::LF => "\u{000A}",
            #[cfg(feature = "unicode-lines")]
            Self::VT => "\u{000B}",
            #[cfg(feature = "unicode-lines")]
            Self::FF => "\u{000C}",
            #[cfg(feature = "unicode-lines")]
            Self::CR => "\u{000D}",
            #[cfg(feature = "unicode-lines")]
            Self::Nel => "\u{0085}",
            #[cfg(feature = "unicode-lines")]
            Self::LS => "\u{2028}",
            #[cfg(feature = "unicode-lines")]
            Self::PS => "\u{2029}",
        }
    }

    #[inline]
    pub const fn from_char(ch: char) -> Option<LineEnding> {
        match ch {
            '\u{000A}' => Some(LineEnding::LF),
            #[cfg(feature = "unicode-lines")]
            '\u{000B}' => Some(LineEnding::VT),
            #[cfg(feature = "unicode-lines")]
            '\u{000C}' => Some(LineEnding::FF),
            #[cfg(feature = "unicode-lines")]
            '\u{000D}' => Some(LineEnding::CR),
            #[cfg(feature = "unicode-lines")]
            '\u{0085}' => Some(LineEnding::Nel),
            #[cfg(feature = "unicode-lines")]
            '\u{2028}' => Some(LineEnding::LS),
            #[cfg(feature = "unicode-lines")]
            '\u{2029}' => Some(LineEnding::PS),
            // Not a line ending
            _ => None,
        }
    }

    // Normally we'd want to implement the FromStr trait, but in this case
    // that would force us into a different return type than from_char,
    // which would be weird.
    #[allow(clippy::should_implement_trait)]
    #[inline]
    pub fn from_str(g: &str) -> Option<LineEnding> {
        match g {
            "\u{000D}\u{000A}" => Some(LineEnding::Crlf),
            "\u{000A}" => Some(LineEnding::LF),
            #[cfg(feature = "unicode-lines")]
            "\u{000B}" => Some(LineEnding::VT),
            #[cfg(feature = "unicode-lines")]
            "\u{000C}" => Some(LineEnding::FF),
            #[cfg(feature = "unicode-lines")]
            "\u{000D}" => Some(LineEnding::CR),
            #[cfg(feature = "unicode-lines")]
            "\u{0085}" => Some(LineEnding::Nel),
            #[cfg(feature = "unicode-lines")]
            "\u{2028}" => Some(LineEnding::LS),
            #[cfg(feature = "unicode-lines")]
            "\u{2029}" => Some(LineEnding::PS),
            // Not a line ending
            _ => None,
        }
    }
}

#[inline]
pub fn str_is_line_ending(s: &str) -> bool {
    LineEnding::from_str(s).is_some()
}

#[inline]
pub fn rope_is_line_ending(r: RopeSlice) -> bool {
    r.chunks().all(str_is_line_ending)
}

/// Attempts to detect what line ending the passed document uses.
pub fn auto_detect_line_ending(doc: &Rope) -> Option<LineEnding> {
    // Return first matched line ending. Not all possible line endings
    // are being matched, as they might be special-use only
    for line in doc.lines(stdx::rope::LINE_TYPE).take(100) {
        match get_line_ending(&line) {
            None => {}
            #[cfg(feature = "unicode-lines")]
            Some(LineEnding::VT) | Some(LineEnding::FF) | Some(LineEnding::PS) => {}
            ending => return ending,
        }
    }
    None
}

/// Returns the passed line's line ending, if any.
pub fn get_line_ending(line: &RopeSlice) -> Option<LineEnding> {
    // Last character as str.
    let g1 = line
        .slice_chars(line.len_chars().saturating_sub(1)..)
        .as_str()
        .unwrap();

    // Last two characters as str, or empty str if they're not contiguous.
    // It's fine to punt on the non-contiguous case, because Ropey guarantees
    // that CRLF is always contiguous.
    let g2 = line
        .slice_chars(line.len_chars().saturating_sub(2)..)
        .as_str()
        .unwrap_or("");

    // First check the two-character case for CRLF, then check the single-character case.
    LineEnding::from_str(g2).or_else(|| LineEnding::from_str(g1))
}

#[cfg(not(feature = "unicode-lines"))]
/// Returns the passed line's line ending, if any.
pub fn get_line_ending_of_str(line: &str) -> Option<LineEnding> {
    if line.ends_with("\u{000D}\u{000A}") {
        Some(LineEnding::Crlf)
    } else if line.ends_with('\u{000A}') {
        Some(LineEnding::LF)
    } else {
        None
    }
}

#[cfg(feature = "unicode-lines")]
/// Returns the passed line's line ending, if any.
pub fn get_line_ending_of_str(line: &str) -> Option<LineEnding> {
    if line.ends_with("\u{000D}\u{000A}") {
        Some(LineEnding::Crlf)
    } else if line.ends_with('\u{000A}') {
        Some(LineEnding::LF)
    } else if line.ends_with('\u{000B}') {
        Some(LineEnding::VT)
    } else if line.ends_with('\u{000C}') {
        Some(LineEnding::FF)
    } else if line.ends_with('\u{000D}') {
        Some(LineEnding::CR)
    } else if line.ends_with('\u{0085}') {
        Some(LineEnding::Nel)
    } else if line.ends_with('\u{2028}') {
        Some(LineEnding::LS)
    } else if line.ends_with('\u{2029}') {
        Some(LineEnding::PS)
    } else {
        None
    }
}

/// Returns the char index of the end of the given line, not including its line ending.
pub fn line_end_char_index(slice: &RopeSlice, line: usize) -> usize {
    slice.line_to_char_idx(line + 1)
        - get_line_ending(&slice.line(line, stdx::rope::LINE_TYPE))
            .map(|le| le.len_chars())
            .unwrap_or(0)
}

pub fn line_end_byte_index(slice: &RopeSlice, line: usize) -> usize {
    slice.line_to_byte_idx(line + 1, stdx::rope::LINE_TYPE)
        - get_line_ending(&slice.line(line, stdx::rope::LINE_TYPE))
            .map(|le| le.as_str().len())
            .unwrap_or(0)
}

/// Fetches line `line_idx` from the passed rope slice, sans any line ending.
pub fn line_without_line_ending<'a>(slice: &'a RopeSlice, line_idx: usize) -> RopeSlice<'a> {
    let start = slice.line_to_byte_idx(line_idx, stdx::rope::LINE_TYPE);
    let end = line_end_byte_index(slice, line_idx);
    slice.slice(start..end)
}

#[cfg(test)]
mod line_ending_tests {
    use super::*;

    #[test]
    fn normalization_matches_regex_for_mixed_endings_and_unicode() {
        let reference = regex::Regex::new(r"\r\n|\r|\n").unwrap();
        let mut inputs = vec![String::new()];
        for _ in 0..5 {
            inputs = inputs
                .iter()
                .flat_map(|prefix| {
                    ["x", "\r", "\n", "é😀", "\u{2028}", "$1"]
                        .map(|suffix| format!("{prefix}{suffix}"))
                })
                .collect();
            for text in &inputs {
                for ending in [
                    LineEnding::LF,
                    LineEnding::Crlf,
                    #[cfg(feature = "unicode-lines")]
                    LineEnding::VT,
                    #[cfg(feature = "unicode-lines")]
                    LineEnding::FF,
                    #[cfg(feature = "unicode-lines")]
                    LineEnding::CR,
                    #[cfg(feature = "unicode-lines")]
                    LineEnding::Nel,
                    #[cfg(feature = "unicode-lines")]
                    LineEnding::LS,
                    #[cfg(feature = "unicode-lines")]
                    LineEnding::PS,
                ] {
                    assert_eq!(
                        normalize_line_endings(text, ending),
                        reference.replace_all(text, ending.as_str()),
                        "{text:?} {ending:?}"
                    );
                }
            }
        }
        for text in ["", "no endings", "é😀\u{2028}\u{85}"] {
            assert!(matches!(
                normalize_line_endings(text, LineEnding::LF),
                Cow::Borrowed(_)
            ));
        }
    }

    #[test]
    fn normalize_mixed_line_endings_preserves_other_text() {
        let input = "é$1\r\n界\rthree\nfour\u{2028}five";
        assert_eq!(
            normalize_line_endings(input, LineEnding::LF),
            "é$1\n界\nthree\nfour\u{2028}five"
        );
        assert_eq!(
            normalize_line_endings(input, LineEnding::Crlf),
            "é$1\r\n界\r\nthree\r\nfour\u{2028}five"
        );
        assert_eq!(normalize_line_endings("", LineEnding::LF), "");
        assert_eq!(normalize_line_endings("text", LineEnding::Crlf), "text");
    }

    #[test]
    fn line_ending_autodetect() {
        assert_eq!(
            auto_detect_line_ending(&Rope::from_str("\n")),
            Some(LineEnding::LF)
        );
        assert_eq!(
            auto_detect_line_ending(&Rope::from_str("\r\n")),
            Some(LineEnding::Crlf)
        );
        assert_eq!(auto_detect_line_ending(&Rope::from_str("hello")), None);
        assert_eq!(auto_detect_line_ending(&Rope::from_str("")), None);
        assert_eq!(
            auto_detect_line_ending(&Rope::from_str("hello\nmitos\r\n")),
            Some(LineEnding::LF)
        );
        assert_eq!(
            auto_detect_line_ending(&Rope::from_str("a formfeed\u{000C}")),
            None
        );
        assert_eq!(
            auto_detect_line_ending(&Rope::from_str("\n\u{000A}\n \u{000A}")),
            Some(LineEnding::LF)
        );
        assert_eq!(
            auto_detect_line_ending(&Rope::from_str(
                "a formfeed\u{000C} with a\u{000C} linefeed\u{000A}"
            )),
            Some(LineEnding::LF)
        );
        assert_eq!(auto_detect_line_ending(&Rope::from_str("a formfeed\u{000C} with a\u{000C} carriage return linefeed\u{000D}\u{000A} and a linefeed\u{000A}")), Some(LineEnding::Crlf));
    }

    #[test]
    fn str_to_line_ending() {
        #[cfg(feature = "unicode-lines")]
        assert_eq!(LineEnding::from_str("\r"), Some(LineEnding::CR));
        assert_eq!(LineEnding::from_str("\n"), Some(LineEnding::LF));
        assert_eq!(LineEnding::from_str("\r\n"), Some(LineEnding::Crlf));
        assert_eq!(LineEnding::from_str("hello\n"), None);
    }

    #[test]
    fn get_line_ending_rope_slice() {
        let r = Rope::from_str("Hello\rworld\nhow\r\nare you?");
        #[cfg(feature = "unicode-lines")]
        assert_eq!(get_line_ending(&r.slice_chars(..6)), Some(LineEnding::CR));
        assert_eq!(get_line_ending(&r.slice_chars(..12)), Some(LineEnding::LF));
        assert_eq!(
            get_line_ending(&r.slice_chars(..17)),
            Some(LineEnding::Crlf)
        );
        assert_eq!(get_line_ending(&r.slice(..)), None);
    }

    #[test]
    fn get_line_ending_str() {
        let text = "Hello\rworld\nhow\r\nare you?";
        #[cfg(feature = "unicode-lines")]
        assert_eq!(get_line_ending_of_str(&text[..6]), Some(LineEnding::CR));
        assert_eq!(get_line_ending_of_str(&text[..12]), Some(LineEnding::LF));
        assert_eq!(get_line_ending_of_str(&text[..17]), Some(LineEnding::Crlf));
        assert_eq!(get_line_ending_of_str(text), None);
    }

    #[test]
    fn line_end_char_index_rope_slice() {
        let r = Rope::from_str("Hello\rworld\nhow\r\nare you?");
        let s = &r.slice(..);
        #[cfg(not(feature = "unicode-lines"))]
        {
            assert_eq!(line_end_char_index(s, 0), 11);
            assert_eq!(line_end_char_index(s, 1), 15);
            assert_eq!(line_end_char_index(s, 2), 25);
        }
        #[cfg(feature = "unicode-lines")]
        {
            assert_eq!(line_end_char_index(s, 0), 5);
            assert_eq!(line_end_char_index(s, 1), 11);
            assert_eq!(line_end_char_index(s, 2), 15);
        }
    }
}

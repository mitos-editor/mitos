//! Text-only indentation detection and normalization.
use crate::{
    chars::{char_is_line_ending, char_is_whitespace},
    graphemes::tab_width_at,
    Rope, RopeSlice, Tendril,
};

/// Enum representing indentation style.
///
/// Only values 1-8 are valid for the `Spaces` variant.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum IndentStyle {
    Tabs,
    Spaces(u8),
}

// 16 spaces
const INDENTS: &str = "                ";
pub const MAX_INDENT: u8 = 16;

impl IndentStyle {
    /// Creates an `IndentStyle` from an indentation string.
    ///
    /// For example, passing `"    "` (four spaces) will create `IndentStyle::Spaces(4)`.
    #[allow(clippy::should_implement_trait)]
    #[inline]
    pub fn from_str(indent: &str) -> Self {
        // XXX: do we care about validating the input more than this?  Probably not...?
        debug_assert!(!indent.is_empty() && indent.len() <= MAX_INDENT as usize);

        if indent.starts_with(' ') {
            IndentStyle::Spaces(indent.len().clamp(1, MAX_INDENT as usize) as u8)
        } else {
            IndentStyle::Tabs
        }
    }

    #[inline]
    pub fn as_str(&self) -> &'static str {
        match *self {
            IndentStyle::Tabs => "\t",
            IndentStyle::Spaces(n) => {
                // Unsupported indentation style.  This should never happen,
                debug_assert!(n > 0 && n <= MAX_INDENT);

                // Either way, clamp to the nearest supported value
                let closest_n = n.clamp(1, MAX_INDENT) as usize;
                &INDENTS[0..closest_n]
            }
        }
    }

    #[inline]
    pub fn indent_width(&self, tab_width: usize) -> usize {
        match *self {
            IndentStyle::Tabs => tab_width,
            IndentStyle::Spaces(width) => width as usize,
        }
    }
}

/// Attempts to detect the indentation style used in a document.
///
/// Returns the indentation style if the auto-detect confidence is
/// reasonably high, otherwise returns `None`.
pub fn auto_detect_indent_style(document_text: &Rope) -> Option<IndentStyle> {
    // Build a histogram of the indentation *increases* between
    // subsequent lines, ignoring lines that are all whitespace.
    //
    // Index 0 is for tabs, the rest are 1-MAX_INDENT spaces.
    let histogram: [usize; MAX_INDENT as usize + 1] = {
        let mut histogram = [0; MAX_INDENT as usize + 1];
        let mut prev_line_is_tabs = false;
        let mut prev_line_leading_count = 0usize;

        // Loop through the lines, checking for and recording indentation
        // increases as we go.
        'outer: for line in document_text.lines().take(1000) {
            let mut c_iter = line.chars();

            // Is first character a tab or space?
            let is_tabs = match c_iter.next() {
                Some('\t') => true,
                Some(' ') => false,

                // Ignore blank lines.
                Some(c) if char_is_line_ending(c) => continue,

                _ => {
                    prev_line_is_tabs = false;
                    prev_line_leading_count = 0;
                    continue;
                }
            };

            // Count the line's total leading tab/space characters.
            let mut leading_count = 1;
            let mut count_is_done = false;
            for c in c_iter {
                match c {
                    '\t' if is_tabs && !count_is_done => leading_count += 1,
                    ' ' if !is_tabs && !count_is_done => leading_count += 1,

                    // We stop counting if we hit whitespace that doesn't
                    // qualify as indent or doesn't match the leading
                    // whitespace, but we don't exit the loop yet because
                    // we still want to determine if the line is blank.
                    c if char_is_whitespace(c) => count_is_done = true,

                    // Ignore blank lines.
                    c if char_is_line_ending(c) => continue 'outer,

                    _ => break,
                }

                // Bound the worst-case execution time for weird text files.
                if leading_count > 256 {
                    continue 'outer;
                }
            }

            // If there was an increase in indentation over the previous
            // line, update the histogram with that increase.
            if (prev_line_is_tabs == is_tabs || prev_line_leading_count == 0)
                && prev_line_leading_count < leading_count
            {
                if is_tabs {
                    histogram[0] += 1;
                } else {
                    let amount = leading_count - prev_line_leading_count;
                    if amount <= MAX_INDENT as usize {
                        histogram[amount] += 1;
                    }
                }
            }

            // Store this line's leading whitespace info for use with
            // the next line.
            prev_line_is_tabs = is_tabs;
            prev_line_leading_count = leading_count;
        }

        // Give more weight to tabs, because their presence is a very
        // strong indicator.
        histogram[0] *= 2;
        // Gives less weight to single indent, as single spaces are
        // often used in certain languages' comment systems and rarely
        // used as the actual document indentation.
        if histogram[1] > 1 {
            histogram[1] /= 2;
        }

        histogram
    };

    // Find the most frequent indent, its frequency, and the frequency of
    // the next-most frequent indent.
    let indent = histogram
        .iter()
        .enumerate()
        .max_by_key(|kv| kv.1)
        .unwrap()
        .0;
    let indent_freq = histogram[indent];
    let indent_freq_2 = *histogram
        .iter()
        .enumerate()
        .filter(|kv| kv.0 != indent)
        .map(|kv| kv.1)
        .max()
        .unwrap();

    // Return the auto-detected result if we're confident enough in its
    // accuracy, based on some heuristics.
    if indent_freq >= 1 && (indent_freq_2 as f64 / indent_freq as f64) < 0.66 {
        Some(match indent {
            0 => IndentStyle::Tabs,
            _ => IndentStyle::Spaces(indent as u8),
        })
    } else {
        None
    }
}

/// To determine indentation of a newly inserted line, figure out the indentation at the last col
/// of the previous line.
pub fn indent_level_for_line(line: RopeSlice, tab_width: usize, indent_width: usize) -> usize {
    let mut len = 0;
    for ch in line.chars() {
        match ch {
            '\t' => len += tab_width_at(len, tab_width as u16),
            ' ' => len += 1,
            _ => break,
        }
    }

    len / indent_width
}

/// normalizes indentation to tabs/spaces based on user configuration
/// This function does not change the actual indentation width, just the character
/// composition.
pub fn normalize_indentation(
    prefix: RopeSlice<'_>,
    line: RopeSlice<'_>,
    dst: &mut Tendril,
    indent_style: IndentStyle,
    tab_width: usize,
) -> usize {
    #[allow(deprecated)]
    let off = crate::visual_coords_at_pos(prefix, prefix.len_chars(), tab_width).col;
    let mut len = 0;
    let mut original_len = 0;
    for ch in line.chars() {
        match ch {
            '\t' => len += tab_width_at(len + off, tab_width as u16),
            ' ' => len += 1,
            _ => break,
        }
        original_len += 1;
    }
    if indent_style == IndentStyle::Tabs {
        dst.extend(std::iter::repeat_n('\t', len / tab_width));
        len %= tab_width;
    }
    dst.extend(std::iter::repeat_n(' ', len));
    original_len
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::Rope;

    #[test]
    fn test_indent_level() {
        let tab_width = 4;
        let indent_width = 4;
        let line = Rope::from("        fn new"); // 8 spaces
        assert_eq!(
            indent_level_for_line(line.slice(..), tab_width, indent_width),
            2
        );
        let line = Rope::from("\t\t\tfn new"); // 3 tabs
        assert_eq!(
            indent_level_for_line(line.slice(..), tab_width, indent_width),
            3
        );
        // mixed indentation
        let line = Rope::from("\t    \tfn new"); // 1 tab, 4 spaces, tab
        assert_eq!(
            indent_level_for_line(line.slice(..), tab_width, indent_width),
            3
        );
    }

    #[test]
    fn test_large_indent_level() {
        let tab_width = 16;
        let indent_width = 16;
        let line = Rope::from("                fn new"); // 16 spaces
        assert_eq!(
            indent_level_for_line(line.slice(..), tab_width, indent_width),
            1
        );
        let line = Rope::from("                                fn new"); // 32 spaces
        assert_eq!(
            indent_level_for_line(line.slice(..), tab_width, indent_width),
            2
        );
    }
}

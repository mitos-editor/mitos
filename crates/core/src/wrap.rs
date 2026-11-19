use smartstring::{LazyCompact, SmartString};
use textwrap::{Options, WordSplitter::NoHyphenation};

/// Given a slice of text, return the text re-wrapped to fit it
/// within the given width.
pub fn reflow_hard_wrap(text: &str, text_width: usize) -> SmartString<LazyCompact> {
    let options = Options::new(text_width)
        .word_splitter(NoHyphenation)
        .word_separator(textwrap::WordSeparator::AsciiSpace);
    textwrap::refill(text, options).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflow_preserves_word_boundaries_indentation_and_display_width() {
        for (text, width, expected) in [
            ("one two\nthree", 7, "one two\nthree"),
            ("  one two\n  three", 9, "  one two\n  three"),
            ("界界 界界", 5, "界界\n界界"),
            ("one\u{a0}two three", 8, "one\u{a0}two\nthree"),
            ("foo-bar baz", 7, "foo-bar\nbaz"),
            ("", 0, ""),
        ] {
            assert_eq!(reflow_hard_wrap(text, width), expected, "{text:?}");
        }
    }
}

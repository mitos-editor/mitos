//! Text scanning for spelling diagnostics.
//!
//! Adapted from Michael Davis's [Helix PR #15910](https://github.com/helix-editor/helix/pull/15910),
//! revision `1849ccc29792484a1e00bcc48ee8018ca058cf33`.

use std::{borrow::Cow, ops::Range, sync::LazyLock};

use editor_core::{
    chars::char_is_word,
    diagnostic::{Diagnostic, Range as DiagnosticRange, Severity},
    syntax::{config::SpellingFilter, Loader},
    RopeSlice, Syntax,
};
use stdx::rope::{Regex, RopeSliceExt as _};
use view::Dictionary;

use super::{MAX_INCREMENTAL_CHARS, PROVIDER};

/// The char ranges within `region` to spell-check. With a syntax tree, checking is restricted to
/// the natural-language regions selected by each layer's `spellcheck.scm` query (comments, prose,
/// ...); without a tree (plain text), the whole `region` is checked.
//
// `Syntax::spell_regions` works in byte offsets (tree-sitter's native unit) while the spelling
// diagnostics, like all diagnostics, are in char offsets, so we convert at this boundary. The
// conversions go away once diagnostics move to byte offsets.
pub(super) fn spell_check_regions(
    syntax: Option<&Syntax>,
    loader: &Loader,
    text: RopeSlice,
    region: Range<usize>,
) -> Vec<Range<usize>> {
    let Some(syntax) = syntax else {
        return vec![region];
    };
    let bytes = text.char_to_byte(region.start)..text.char_to_byte(region.end);
    syntax
        .spell_regions(text, loader, bytes)
        .into_iter()
        .map(|region| text.byte_to_char(region.start)..text.byte_to_char(region.end))
        .collect()
}

/// Include whole tokens and URL/email spans at incremental window edges. Very long tokens
/// must be scanned off the editor thread, even if the edit itself was small.
pub(super) fn expand_check_window(
    text: RopeSlice,
    mut range: Range<usize>,
) -> Option<Range<usize>> {
    if range.len() > MAX_INCREMENTAL_CHARS {
        return None;
    }
    while range.start > 0 && !text.char(range.start - 1).is_whitespace() {
        if range.len() == MAX_INCREMENTAL_CHARS {
            return None;
        }
        range.start -= 1;
    }
    while range.end < text.len_chars() && !text.char(range.end).is_whitespace() {
        if range.len() == MAX_INCREMENTAL_CHARS {
            return None;
        }
        range.end += 1;
    }
    Some(range)
}

/// Tokenizes the `region` (a char range) of `text` and appends a diagnostic for each word that
/// every dictionary rejects (a word known to any one of them is accepted). Match offsets from
/// `regex_input_at` are absolute byte offsets in `text`.
pub(super) fn check_region(
    dictionaries: &[&Dictionary],
    filter: &SpellingFilter,
    text: RopeSlice,
    region: Range<usize>,
    out: &mut Vec<Diagnostic>,
    mut is_canceled: impl FnMut() -> bool,
) {
    static WORDS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"[\p{L}\p{N}_][\p{L}\p{M}\p{N}_]*(?:['’-][\p{L}\p{M}\p{N}_]+)*").unwrap()
    });
    // URLs and email addresses tokenize into word-like fragments (host and path segments) that
    // aren't real words, so skip any word overlapping one. These match the source text rather than
    // individual tokens, which is why they can't be expressed as ordinary `ignore-regexes`.
    static IGNORE_SPANS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"[a-zA-Z][a-zA-Z0-9+.-]*://\S+|[\w.+-]+@[A-Za-z0-9-]+\.[\w.-]+").unwrap()
    });

    let mut ignore_spans = IGNORE_SPANS
        .find_iter(text.regex_input_at(region.clone()))
        .peekable();

    for m in WORDS.find_iter(text.regex_input_at(region)) {
        if is_canceled() {
            break;
        }
        // Both iterators are ordered. Advance past earlier spans once instead of comparing each
        // word against every URL/email in the region (quadratic on link-heavy documents).
        while ignore_spans
            .peek()
            .is_some_and(|span| span.end() <= m.start())
        {
            ignore_spans.next();
        }
        if ignore_spans
            .peek()
            .is_some_and(|span| span.start() < m.end())
        {
            continue;
        }
        let word = Cow::from(text.byte_slice(m.range()));
        // Honor explicit ignores and dictionary entries for the complete token before splitting
        // identifiers or compounds. This also preserves accepted contractions and hyphenation.
        if filter.ignores(&word)
            || dictionaries
                .iter()
                .any(|dictionary| dictionary.check(&word))
        {
            continue;
        }
        for (index, (offset, part)) in word_parts(&word).enumerate() {
            if index > 0 && is_canceled() {
                return;
            }
            if part != word
                && (filter.ignores(part)
                    || dictionaries.iter().any(|dictionary| dictionary.check(part)))
            {
                continue;
            }
            let start = text.byte_to_char(m.start() + offset);
            let end = text.byte_to_char(m.start() + offset + part.len());
            out.push(spelling_diagnostic(text, start, end, part));
        }
    }
}

/// Split separators and digits, then camelCase/PascalCase and acronym boundaries (HTTPServer).
/// Keep apostrophes and combining marks attached, and return byte offsets into the original token.
fn word_parts(word: &str) -> impl Iterator<Item = (usize, &str)> {
    static PARTS: LazyLock<editor_core::regex::Regex> = LazyLock::new(|| {
        editor_core::regex::Regex::new(r"\p{L}[\p{L}\p{M}]*(?:['’][\p{L}\p{M}]+)*").unwrap()
    });
    PARTS.find_iter(word).flat_map(|part| {
        use editor_core::unicode::category::{get_general_category, GeneralCategory};

        let mut chars = part
            .as_str()
            .char_indices()
            .filter(|&(_, ch)| {
                !matches!(
                    get_general_category(ch),
                    GeneralCategory::NonspacingMark
                        | GeneralCategory::SpacingMark
                        | GeneralCategory::EnclosingMark
                )
            })
            .peekable();
        let mut previous = ' ';
        let mut start = 0;
        std::iter::from_fn(move || {
            while let Some((offset, ch)) = chars.next() {
                let boundary = ch.is_uppercase()
                    && (previous.is_lowercase()
                        || (previous.is_uppercase()
                            && chars.peek().is_some_and(|&(_, next)| next.is_lowercase())));
                previous = ch;
                if boundary {
                    let result = (part.start() + start, &part.as_str()[start..offset]);
                    start = offset;
                    return Some(result);
                }
            }
            if start == part.len() {
                return None;
            }
            let result = (part.start() + start, &part.as_str()[start..]);
            start = part.len();
            Some(result)
        })
    })
}

fn spelling_diagnostic(text: RopeSlice, start: usize, end: usize, word: &str) -> Diagnostic {
    // Mirror `lsp_diagnostic_to_diagnostic` so edit-mapping associations behave the same.
    let ends_at_word = start != end && end != 0 && text.get_char(end - 1).is_some_and(char_is_word);
    let starts_at_word = start != end && text.get_char(start).is_some_and(char_is_word);
    Diagnostic {
        range: DiagnosticRange { start, end },
        ends_at_word,
        starts_at_word,
        zero_width: start == end,
        line: text.char_to_line(start),
        message: format!("Possible spelling mistake: '{word}'").into(),
        severity: Some(Severity::Hint),
        code: None,
        provider: PROVIDER,
        tags: Vec::new(),
        source: Some("spelling".into()),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{diagnostic::DiagnosticProvider, syntax::config::SpellingConfig, Rope};

    /// The `en_US` dictionary vendored under `runtime/dictionaries/`.
    fn en_us() -> Dictionary {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../runtime/dictionaries/en_US"
        );
        let aff = std::fs::read_to_string(format!("{dir}/en_US.aff")).unwrap();
        let dic = std::fs::read_to_string(format!("{dir}/en_US.dic")).unwrap();
        Dictionary::new(&aff, &dic).unwrap()
    }

    /// A filter that skips nothing (the default config).
    fn no_filter() -> SpellingFilter {
        SpellingFilter::new(&SpellingConfig::default())
    }

    fn check(text: &str, region: Range<usize>) -> Vec<Diagnostic> {
        let rope = Rope::from_str(text);
        let mut out = Vec::new();
        check_region(
            &[&en_us()],
            &no_filter(),
            rope.slice(..),
            region,
            &mut out,
            || false,
        );
        out
    }

    /// A throwaway dictionary containing exactly `words`, for testing multi-dictionary checks.
    fn mini_dictionary(words: &[&str]) -> Dictionary {
        let dic = format!("{}\n{}\n", words.len(), words.join("\n"));
        Dictionary::new("SET UTF-8\n", &dic).unwrap()
    }

    #[test]
    fn splits_identifier_styles_and_preserves_prose() {
        for (source, expected) in [
            ("__snake__case_", vec!["snake", "case"]),
            ("SCREAMING_SNAKE_CASE", vec!["SCREAMING", "SNAKE", "CASE"]),
            (
                "camelCase PascalCase",
                vec!["camel", "Case", "Pascal", "Case"],
            ),
            (
                "HTTPServer parseXML",
                vec!["HTTP", "Server", "parse", "XML"],
            ),
            ("version2Value_42test 123", vec!["version", "Value", "test"]),
            (
                "well-known don't isn’t",
                vec!["well", "known", "don't", "isn’t"],
            ),
            (
                "naïveÉcole cafe\u{301}Value",
                vec!["naïve", "École", "cafe\u{301}", "Value"],
            ),
            ("HTT\u{301}PServer", vec!["HTT\u{301}P", "Server"]),
            ("привет_мир 中文", vec!["привет", "мир", "中文"]),
            ("___123___", vec![]),
        ] {
            let parts: Vec<_> = word_parts(source)
                .map(|(offset, part)| {
                    assert_eq!(&source[offset..offset + part.len()], part);
                    part
                })
                .collect();
            assert_eq!(parts, expected, "{source:?}");
        }
    }

    #[test]
    fn identifier_findings_cover_only_misspelled_parts() {
        let text = "🚀 hello_quik helloQuik HelloQuik HTTPQuik hello2quik well-quik";
        let diagnostics = check(text, 0..text.chars().count());
        let rope = Rope::from_str(text);
        let words: Vec<_> = diagnostics
            .iter()
            .map(|d| rope.slice(d.range.start..d.range.end).to_string())
            .collect();
        assert_eq!(words, ["quik", "Quik", "Quik", "Quik", "quik", "quik"]);
        assert_eq!(diagnostics[0].range.start, 8);
    }

    #[test]
    fn filters_and_dictionaries_accept_whole_tokens_and_parts() {
        let dictionary = mini_dictionary(&["hello", "iPhone", "quik-wrld", "don't", "isn’t"]);
        let filter = SpellingFilter::new(&SpellingConfig {
            words: vec!["custom_token".into(), "allow".into()],
            ignore_regexes: vec!["^[A-Z0-9_]+$".into(), "^skip_".into(), "^ignore$".into()],
            min_word_length: Some(3),
            ..Default::default()
        });
        let text = Rope::from_str("custom_token CUSTOM_TOKEN skip_wrld WRLD_VALUE hello_allow hello_ignore hello_xy iPhone quik-wrld don't isn’t hello_wrld");
        let mut diagnostics = Vec::new();
        check_region(
            &[&dictionary],
            &filter,
            text.slice(..),
            0..text.len_chars(),
            &mut diagnostics,
            || false,
        );
        let words: Vec<_> = diagnostics
            .iter()
            .map(|d| text.slice(d.range.start..d.range.end).to_string())
            .collect();
        assert_eq!(words, ["wrld"]);
    }

    #[test]
    fn a_word_known_to_any_dictionary_is_accepted() {
        // "wrld" is not in en_US, so en_US alone flags it.
        let rope = Rope::from_str("wrld");
        let mut out = Vec::new();
        check_region(
            &[&en_us()],
            &no_filter(),
            rope.slice(..),
            0..4,
            &mut out,
            || false,
        );
        assert_eq!(out.len(), 1, "{out:?}");

        // A second dictionary that knows "wrld" makes the OR accept it.
        let custom = mini_dictionary(&["wrld"]);
        let mut out = Vec::new();
        check_region(
            &[&en_us(), &custom],
            &no_filter(),
            rope.slice(..),
            0..4,
            &mut out,
            || false,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn flags_only_the_misspelled_word() {
        let diagnostics = check("the quik brown fox", 0..18);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let d = &diagnostics[0];
        assert_eq!((d.range.start, d.range.end), (4, 8));
        assert_eq!(d.provider, DiagnosticProvider::Spelling);
        assert_eq!(d.severity, Some(Severity::Hint));
        assert!(d.starts_at_word && d.ends_at_word && !d.zero_width);
    }

    #[test]
    fn region_scopes_the_scan() {
        // The same misspelling appears twice; only the one inside the region is reported.
        let diagnostics = check("quik brown quik", 11..15);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(
            (diagnostics[0].range.start, diagnostics[0].range.end),
            (11, 15)
        );
    }

    #[test]
    fn offsets_are_char_indices_across_multibyte_text() {
        // A 4-byte emoji precedes the misspelling: the diagnostic range must be in chars (2..6),
        // not bytes (5..9), exercising the byte→char conversion.
        let diagnostics = check("🚀 quik", 0..6);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(
            (diagnostics[0].range.start, diagnostics[0].range.end),
            (2, 6)
        );
    }

    #[test]
    fn skips_words_inside_urls_and_emails() {
        // Only the prose misspelling "teh" is flagged; identifier parts inside URLs and emails
        // are skipped along with the misspelled-looking host names.
        let text = "teh https://github.com/foo/barBaz_quik me_quik@exampel.org";
        let diagnostics = check(text, 0..text.len());
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(diagnostics[0].message.contains("teh"), "{diagnostics:?}");
    }

    #[test]
    fn filter_skips_allowlisted_short_and_ignored_words() {
        let config = SpellingConfig {
            words: vec!["Helix".into()],
            ignore_regexes: vec!["^[A-Z0-9_]+$".into()],
            min_word_length: Some(3),
            ..Default::default()
        };
        let filter = SpellingFilter::new(&config);
        let rope = Rope::from_str("Helix HE teh ABC123");
        let mut out = Vec::new();
        check_region(
            &[&en_us()],
            &filter,
            rope.slice(..),
            0..rope.len_chars(),
            &mut out,
            || false,
        );
        // "Helix" is allowlisted (case-insensitively), "HE" is too short, and "ABC123" matches the
        // ignore regex; only the genuine misspelling "teh" survives.
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(
            rope.slice(out[0].range.start..out[0].range.end).to_string(),
            "teh"
        );
    }

    /// Runs the full-document check path's logic (region selection + tokenization) against a real
    /// syntax tree, the way `check_text` does off-thread.
    fn check_scoped(language: &str, text: &str) -> Vec<Diagnostic> {
        let loader = editor_core::config::default_lang_loader();
        let rope = Rope::from_str(text);
        let language = loader.language_for_name(language).unwrap();
        let syntax = Syntax::new(rope.slice(..), language, &loader).unwrap();
        let dictionary = en_us();
        let mut out = Vec::new();
        for region in
            spell_check_regions(Some(&syntax), &loader, rope.slice(..), 0..rope.len_chars())
        {
            check_region(
                &[&dictionary],
                &no_filter(),
                rope.slice(..),
                region,
                &mut out,
                || false,
            );
        }
        out
    }

    #[test]
    fn syntax_scoping_checks_comments_not_code() {
        // `teh` in the comment is a misspelling; the identically misspelled identifier `teh_value`
        // is code and must not be flagged.
        let diagnostics = check_scoped("rust", "// teh_bug\nlet teh_value = 1;\n");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(diagnostics[0].message.contains("teh"), "{diagnostics:?}");
        assert_eq!(diagnostics[0].range.start, 3, "the comment occurrence");
    }

    #[test]
    fn checks_complete_unicode_words_against_every_dictionary() {
        let english = Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap();
        let multilingual =
            Dictionary::new("SET UTF-8\n", "4\nGrüße\nnaïve\ncafe\u{301}\nпривет\n").unwrap();
        let text = Rope::from_str("hello Grüße naïve cafe\u{301} привеет");
        let mut diagnostics = Vec::new();
        check_region(
            &[&english, &multilingual],
            &no_filter(),
            text.slice(..),
            0..text.len_chars(),
            &mut diagnostics,
            || false,
        );
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let range = diagnostics[0].range;
        assert_eq!(text.slice(range.start..range.end).to_string(), "привеет");
    }

    #[test]
    fn incremental_window_includes_complete_urls_and_words() {
        let source = format!("hello https://exampel.org/{} quik", "x".repeat(100));
        let text = Rope::from_str(&source);
        let region = expand_check_window(text.slice(..), 50..text.len_chars()).unwrap();
        assert_eq!(region.start, 6);
        let mut diagnostics = Vec::new();
        check_region(
            &[&en_us()],
            &no_filter(),
            text.slice(..),
            region,
            &mut diagnostics,
            || false,
        );
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let range = diagnostics[0].range;
        assert_eq!(text.slice(range.start..range.end).to_string(), "quik");
    }

    #[test]
    fn oversized_windows_require_a_background_scan() {
        let text = Rope::from_str(&"x".repeat(MAX_INCREMENTAL_CHARS * 3));
        assert!(expand_check_window(text.slice(..), 0..50).is_none());
        assert!(expand_check_window(text.slice(..), 1500..1550).is_none());
        assert!(expand_check_window(text.slice(..), 0..text.len_chars()).is_none());
    }

    #[test]
    fn cancellation_stops_tokenization() {
        let text = Rope::from_str("teh quik wrld");
        let dictionary = mini_dictionary(&["hello"]);
        let mut diagnostics = Vec::new();
        let mut checked = 0;
        check_region(
            &[&dictionary],
            &no_filter(),
            text.slice(..),
            0..text.len_chars(),
            &mut diagnostics,
            || {
                checked += 1;
                checked > 1
            },
        );
        assert_eq!(diagnostics.len(), 1);
        let range = diagnostics[0].range;
        assert_eq!(text.slice(range.start..range.end).to_string(), "teh");
    }

    #[test]
    fn cancellation_stops_within_an_identifier() {
        let text = Rope::from_str("teh_quik_wrld");
        let dictionary = mini_dictionary(&["hello"]);
        let mut diagnostics = Vec::new();
        let mut checked = 0;
        check_region(
            &[&dictionary],
            &no_filter(),
            text.slice(..),
            0..text.len_chars(),
            &mut diagnostics,
            || {
                checked += 1;
                checked > 1
            },
        );
        assert_eq!(diagnostics.len(), 1);
        let range = diagnostics[0].range;
        assert_eq!(text.slice(range.start..range.end).to_string(), "teh");
    }

    #[test]
    fn checks_prose_between_multiple_ignored_spans() {
        let text = "https://exampel.org/foo teh me@exampel.org quik https://exampel.org/bar wrld";
        let diagnostics = check(text, 0..text.len());
        let words: Vec<_> = diagnostics
            .iter()
            .map(|d| &text[d.range.start..d.range.end])
            .collect();
        assert_eq!(words, ["teh", "quik", "wrld"]);
    }
}

//! Unicode word counts, incremental updates, and lexical matching over rope snapshots.
//! Callers own document identities, worker scheduling, and cancellation policy.
//! A canceled update may leave partial counts; discard that index on cancellation.

use editor_core::{chars::char_is_word, Rope, RopeSlice};
use foldhash::HashMap;
use std::{borrow::Cow, iter};
use stdx::rope::RopeExt as _;
use stdx::rope::RopeSliceExt as _;

/// Minimum number of grapheme clusters required to include a word in the index
const MIN_WORD_GRAPHEMES: usize = 3;
/// Maximum word length allowed (in chars)
const MAX_WORD_LEN: usize = 50;
/// Number of input graphemes to scan between cancellation checks.
const CANCEL_CHECK_INTERVAL: usize = 4096;

type Word = editor_core::Tendril;

#[derive(Debug, Default)]
pub struct WordIndex {
    /// Reference counted storage for words.
    ///
    /// Words are very likely to be reused many times. Instead of storing duplicates we keep a
    /// reference count of times a word is used. When the reference count drops to zero the word
    /// is removed from the index.
    words: HashMap<Word, u32>,
}

impl WordIndex {
    /// Distinct words and their reference counts across the indexed texts.
    pub fn word_counts(&self) -> impl Iterator<Item = (&str, u32)> {
        self.words
            .iter()
            .map(|(word, count)| (word.as_str(), *count))
    }

    fn words(&self) -> impl Iterator<Item = &Word> {
        self.words.keys()
    }

    fn insert(&mut self, word: RopeSlice) {
        let word: Cow<str> = word.into();
        if let Some(rc) = self.words.get_mut(word.as_ref()) {
            *rc = rc.saturating_add(1);
        } else {
            let word = Word::from(word);
            self.words.insert(word, 1);
        }
    }

    fn remove(&mut self, word: RopeSlice) {
        let word: Cow<str> = word.into();
        match self.words.get_mut(word.as_ref()) {
            Some(1) => {
                self.words.remove(word.as_ref());
            }
            Some(n) => *n -= 1,
            None => (),
        }
    }

    pub fn clear(&mut self) {
        std::mem::take(&mut self.words);
    }

    pub fn matches(&self, pattern: &str, matcher: &mut fuzzy::Matcher) -> Vec<String> {
        let mut matches = matcher.match_list(pattern, self.words());
        matches.sort_unstable_by_key(|(_, score)| *score);
        matches
            .into_iter()
            .map(|(word, _)| word.to_string())
            .collect()
    }

    pub fn add_document(&mut self, text: &Rope, should_cancel: impl Fn() -> bool) {
        for word in words_cancellable(text.slice(..), &should_cancel) {
            self.insert(word);
        }
    }

    /// Replace indexed word windows, such as the pair returned by [`changed_words`].
    pub fn replace_words(
        &mut self,
        old: RopeSlice,
        text: RopeSlice,
        should_cancel: impl Fn() -> bool,
    ) {
        for word in words_cancellable(text, &should_cancel) {
            self.insert(word);
        }
        for word in words_cancellable(old, &should_cancel) {
            self.remove(word);
        }
    }

    pub fn remove_document(&mut self, text: &Rope, should_cancel: impl Fn() -> bool) {
        for word in words_cancellable(text.slice(..), &should_cancel) {
            self.remove(word);
        }
    }
}

/// Word windows changed between two snapshots, including enough unchanged context
/// to handle words crossing edit boundaries. Returns `None` for equal text or cancellation.
/// Call before locking an index; scanning large unchanged prefixes remains cancellable.
pub fn changed_words<'a>(
    old: RopeSlice<'a>,
    text: RopeSlice<'a>,
    should_cancel: impl Fn() -> bool,
) -> Option<(RopeSlice<'a>, RopeSlice<'a>)> {
    let prefix_bytes = common_bytes(old.chunks(), text.chunks(), false, &should_cancel)?;
    let prefix_bytes = old.floor_char_boundary(prefix_bytes);
    if prefix_bytes == old.len() && prefix_bytes == text.len() {
        return None;
    }
    let prefix = old.byte_to_char_idx(prefix_bytes);
    let suffix_bytes = common_bytes(
        old.slice(prefix_bytes..)
            .chunks_at(old.len() - prefix_bytes)
            .0
            .reversed(),
        text.slice(prefix_bytes..)
            .chunks_at(text.len() - prefix_bytes)
            .0
            .reversed(),
        true,
        &should_cancel,
    )?;
    // Matching byte suffixes can start inside two different UTF-8 characters.
    let suffix_start = old.ceil_char_boundary(old.len() - suffix_bytes);
    let suffix = old.len_chars() - old.byte_to_char_idx(suffix_start);
    if should_cancel() {
        return None;
    }
    let start = prefix.saturating_sub(MAX_WORD_LEN);
    let old_end = (old.len_chars() - suffix + MAX_WORD_LEN).min(old.len_chars());
    let end = (text.len_chars() - suffix + MAX_WORD_LEN).min(text.len_chars());
    Some((
        old.slice_chars(start..old_end),
        text.slice_chars(start..end),
    ))
}

/// Compare bounded byte spans, independently of the two ropes' chunk boundaries.
fn common_bytes<'a>(
    mut before: impl Iterator<Item = &'a str>,
    mut after: impl Iterator<Item = &'a str>,
    reverse: bool,
    cancel: &impl Fn() -> bool,
) -> Option<usize> {
    let mut a = &[][..];
    let mut b = &[][..];
    let mut count = 0;
    loop {
        if a.is_empty() {
            a = before
                .find(|chunk| !chunk.is_empty())
                .unwrap_or("")
                .as_bytes();
        }
        if b.is_empty() {
            b = after
                .find(|chunk| !chunk.is_empty())
                .unwrap_or("")
                .as_bytes();
        }
        let len = a.len().min(b.len()).min(CANCEL_CHECK_INTERVAL);
        if len == 0 {
            return Some(count);
        }
        if cancel() {
            return None;
        }
        let (left, right) = if reverse {
            (&a[a.len() - len..], &b[b.len() - len..])
        } else {
            (&a[..len], &b[..len])
        };
        if left != right {
            let equal = if reverse {
                left.iter()
                    .rev()
                    .zip(right.iter().rev())
                    .take_while(|(a, b)| a == b)
                    .count()
            } else {
                left.iter().zip(right).take_while(|(a, b)| a == b).count()
            };
            return Some(count + equal);
        }
        count += len;
        if reverse {
            a = &a[..a.len() - len];
            b = &b[..b.len() - len];
        } else {
            a = &a[len..];
            b = &b[len..];
        }
    }
}

/// Extracts indexable words from a rope slice.
///
/// A word is a run of grapheme clusters whose first character is a
/// [word character][char_is_word], spanning at least 3 clusters and at most 50
/// Unicode scalar values. All other text is skipped.
///
/// This is a single forward pass over the text's grapheme clusters: each cluster is visited once.
/// and the only rope position ever sought is the start of an emitted word, which keeps
/// extraction roughly linear in the length of the text.
pub fn words(text: RopeSlice) -> impl Iterator<Item = RopeSlice> {
    words_cancellable(text, || false)
}

fn words_cancellable(
    text: RopeSlice,
    should_cancel: impl Fn() -> bool,
) -> impl Iterator<Item = RopeSlice> {
    let mut scanned = 0;
    let mut canceled = false;
    let mut graphemes = text.grapheme_indices();
    // The in-progress word run: the byte offset of its first cluster, its length in chars, and the
    // number of graphemes it spans. `graphemes_len == 0` means we are between words.
    let mut start_byte = 0;
    let mut char_len = 0;
    let mut graphemes_len = 0;

    // Yields `text[start_byte..end_byte]` if that run satisfies the length bounds.
    let qualify = move |start_byte, end_byte, char_len, graphemes_len| {
        (graphemes_len >= MIN_WORD_GRAPHEMES && char_len <= MAX_WORD_LEN)
            .then(|| text.slice(start_byte..end_byte))
    };

    iter::from_fn(move || {
        loop {
            // Check input progress rather than emitted words: punctuation and oversized
            // tokens may scan the entire document without yielding a single word.
            if canceled || (scanned == 0 && should_cancel()) {
                canceled = true;
                return None;
            }
            scanned = (scanned + 1) % CANCEL_CHECK_INTERVAL;
            let Some((byte_idx, grapheme)) = graphemes.next() else {
                // Flush a word that runs up to the end of the text.
                let word = qualify(start_byte, text.len(), char_len, graphemes_len);
                graphemes_len = 0;
                return word;
            };

            if grapheme.chars().next().is_some_and(char_is_word) {
                if graphemes_len == 0 {
                    start_byte = byte_idx;
                    char_len = 0;
                }
                graphemes_len += 1;
                char_len += grapheme.len_chars();
            } else if graphemes_len != 0 {
                // A non-word cluster ends the current run; `byte_idx` is one past the run's end.
                let word = qualify(start_byte, byte_idx, char_len, graphemes_len);
                graphemes_len = 0;
                if word.is_some() {
                    return word;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use quickcheck::{Arbitrary, Gen};

    impl WordIndex {
        fn update(&mut self, old: &Rope, text: &Rope, cancel: impl Fn() -> bool) {
            if let Some((old_words, new_words)) =
                changed_words(old.slice(..), text.slice(..), &cancel)
            {
                self.replace_words(old_words, new_words, cancel);
            }
        }
        fn words_set(&self) -> HashSet<String> {
            self.words.keys().map(ToString::to_string).collect()
        }
        fn counts(&self) -> std::collections::HashMap<String, u32> {
            self.word_counts()
                .map(|(word, count)| (word.to_owned(), count))
                .collect()
        }
    }

    #[track_caller]
    fn assert_words<I: ToString, T: IntoIterator<Item = I>>(text: &str, expected: T) {
        let text = Rope::from_str(text);
        let mut index = WordIndex::default();

        index.add_document(&text, || false);
        let actual = index.words_set();
        let expected: HashSet<_> = expected.into_iter().map(|i| i.to_string()).collect();
        assert_eq!(expected, actual);
    }

    #[test]
    fn snapshot_windows_preserve_word_counts_across_unicode_and_distant_edits() {
        fn check(before: &str, after: &str) {
            let old = Rope::from(before);
            let text = Rope::from(after);
            let mut incremental = WordIndex::default();
            incremental.add_document(&old, || false);
            if let Some((old_words, new_words)) =
                changed_words(old.slice(..), text.slice(..), || false)
            {
                incremental.replace_words(old_words, new_words, || false);
            }
            let mut fresh = WordIndex::default();
            fresh.add_document(&text, || false);
            assert_eq!(incremental.counts(), fresh.counts());
        }
        check("", "αβγ café ábc");
        check("αβγ café ábc", "");
        check("one éclair three", "one èclair three");
        check(
            &format!(
                "{} start middle end {}",
                "long".repeat(30),
                "word".repeat(30)
            ),
            &format!(
                "{} first middle last {}",
                "long".repeat(30),
                "word".repeat(30)
            ),
        );
        check(
            &format!("prefix {} aaa", ".".repeat(200)),
            &format!("changed {} bbb", ".".repeat(200)),
        );
    }

    #[test]
    fn chunk_windows_match_scalar_windows_for_unicode_and_different_chunk_layouts() {
        fn check(old: &Rope, new: &Rope) {
            let a: Vec<_> = old.chars().collect();
            let b: Vec<_> = new.chars().collect();
            let prefix = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
            let actual = changed_words(old.slice(..), new.slice(..), || false);
            if prefix == a.len() && prefix == b.len() {
                assert!(actual.is_none());
                return;
            }
            let suffix = a[prefix..]
                .iter()
                .rev()
                .zip(b[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            let start = prefix.saturating_sub(MAX_WORD_LEN);
            let expected_a: String = a[start..(a.len() - suffix + MAX_WORD_LEN).min(a.len())]
                .iter()
                .collect();
            let expected_b: String = b[start..(b.len() - suffix + MAX_WORD_LEN).min(b.len())]
                .iter()
                .collect();
            let (actual_a, actual_b) = actual.unwrap();
            assert_eq!(actual_a, expected_a);
            assert_eq!(actual_b, expected_b);
        }
        for (left, right) in [
            ("α", "β"),
            ("😀", "😁"),
            ("é", "ĩ"),
            ("", "界"),
            ("abc", ""),
        ] {
            for padding in [0, 1, 127, 1024, 4097] {
                let old = Rope::from(format!(
                    "{}{left}{}",
                    "前a".repeat(padding),
                    "後😀".repeat(padding)
                ));
                let new = Rope::from(format!(
                    "{}{right}{}",
                    "前a".repeat(padding),
                    "後😀".repeat(padding)
                ));
                check(&old, &new);
                check(&old, &old);
                let mut edited = old.clone();
                edited.insert(0, "x");
                check(&old, &edited);
                edited.remove(0..1);
                check(&old, &edited);
            }
        }
    }

    #[test]
    fn snapshot_window_scanning_checks_cancellation_inside_unchanged_text() {
        let before = Rope::from(format!("{} old", "unchanged ".repeat(2000)));
        let after = Rope::from(format!("{} new", "unchanged ".repeat(2000)));
        let checks = std::cell::Cell::new(0);
        let windows = changed_words(before.slice(..), after.slice(..), || {
            checks.set(checks.get() + 1);
            checks.get() == 2
        });
        assert!(windows.is_none());
        assert_eq!(checks.get(), 2);
    }

    #[test]
    fn cancellation_before_scanning_leaves_counts_unchanged() {
        let text = Rope::from("some_word ".repeat(100_000));
        let empty = Rope::new();
        let mut index = WordIndex::default();
        index.add_document(&text, || true);
        assert_eq!(index.word_counts().count(), 0);
        index.update(&empty, &text, || true);
        assert_eq!(index.word_counts().count(), 0);
        index.add_document(&text, || false);
        let counts = index.counts();
        index.remove_document(&text, || true);
        assert_eq!(index.counts(), counts);
    }

    #[test]
    fn cancellation_is_checked_while_scanning_rejected_input() {
        use std::cell::Cell;
        for input in [". ".repeat(100_000), "a".repeat(100_000)] {
            let text = Rope::from(input);
            let checks = Cell::new(0);
            let mut words = words_cancellable(text.slice(..), || {
                checks.set(checks.get() + 1);
                checks.get() == 2
            });
            assert!(words.next().is_none());
            assert_eq!(checks.get(), 2);
            assert!(words.next().is_none());
        }
    }

    #[test]
    fn cancellation_interrupts_a_single_large_changed_window() {
        use std::cell::Cell;
        let text = Rope::from("some_word ".repeat(100_000));
        let empty = Rope::new();
        let checks = Cell::new(0);
        let mut index = WordIndex::default();
        index.update(&empty, &text, || {
            checks.set(checks.get() + 1);
            checks.get() >= 3
        });
        let count = index.word_counts().next().unwrap().1;
        assert!(count > 0 && count < 1_000, "processed {count} words");
    }

    #[test]
    fn parse() {
        assert_words("one two three", ["one", "two", "three"]);
        assert_words("a foo c", ["foo"]);
    }

    #[track_caller]
    fn assert_diff<S, R, I>(before: &str, after: &str, expect_removed: R, expect_inserted: I)
    where
        S: ToString,
        R: IntoIterator<Item = S>,
        I: IntoIterator<Item = S>,
    {
        let before = Rope::from_str(before);
        let after = Rope::from_str(after);
        let expect_removed: HashSet<_> =
            expect_removed.into_iter().map(|i| i.to_string()).collect();
        let expect_inserted: HashSet<_> =
            expect_inserted.into_iter().map(|i| i.to_string()).collect();

        let mut index = WordIndex::default();

        index.add_document(&before, || false);
        let words_before = index.words_set();
        index.update(&before, &after, || false);
        let words_after = index.words_set();

        let actual_removed = words_before.difference(&words_after).cloned().collect();
        let actual_inserted = words_after.difference(&words_before).cloned().collect();

        eprintln!("\"{before}\" {words_before:?} => \"{after}\" {words_after:?}");
        assert_eq!(
            expect_removed, actual_removed,
            "expected {expect_removed:?} to be removed, instead {actual_removed:?} was"
        );
        assert_eq!(
            expect_inserted, actual_inserted,
            "expected {expect_inserted:?} to be inserted, instead {actual_inserted:?} was"
        );
    }

    #[test]
    fn diff() {
        assert_diff("one two three", "one five three", ["two"], ["five"]);
        assert_diff("one two three", "one to three", ["two"], []);
        assert_diff("one two three", "one three", ["two"], []);
        assert_diff("one two three", "one t{o three", ["two"], []);
        assert_diff("one foo three", "one fooo three", ["foo"], ["fooo"]);
    }

    const CORPORA: &[(&str, &str)] = &[
        ("arabic", include_str!("../benches/texts/arabic.txt")),
        ("english", include_str!("../benches/texts/english.txt")),
        ("hindi", include_str!("../benches/texts/hindi.txt")),
        ("japanese", include_str!("../benches/texts/japanese.txt")),
        ("korean", include_str!("../benches/texts/korean.txt")),
        ("mandarin", include_str!("../benches/texts/mandarin.txt")),
        ("russian", include_str!("../benches/texts/russian.txt")),
        (
            "source_code",
            include_str!("../benches/texts/source_code.txt"),
        ),
    ];

    #[track_caller]
    fn assert_extracts(text: &str, expected: &[&str]) {
        let rope = Rope::from_str(text);
        let got: Vec<String> = words(rope.slice(..)).map(|w| w.to_string()).collect();
        assert_eq!(got, expected, "extracting words from {text:?}");
    }

    /// `words` categorizes whole grapheme clusters, so a word boundary never falls inside a
    /// cluster. A non-word combining mark stays attached to the word character it modifies.
    #[test]
    fn extract_respects_grapheme_clusters() {
        // Whitespace and punctuation separate words. Runs under MIN_WORD_GRAPHEMES are dropped.
        assert_extracts("a foo c", &["foo"]);
        assert_extracts(
            "foo.bar.baz qux::quux",
            &["foo", "bar", "baz", "qux", "quux"],
        );
        assert_extracts("snake_case_id CamelCase", &["snake_case_id", "CamelCase"]);
        // Precomposed and decomposed accents both keep the whole word: the combining mark rides
        // along with the letter it attaches to rather than truncating the word before it.
        assert_extracts("naïve café résumé", &["naïve", "café", "résumé"]);
        assert_extracts(
            "nai\u{0308}ve cafe\u{0301}",
            &["nai\u{0308}ve", "cafe\u{0301}"],
        );
        // A Devanagari conjunct is joined by a virama (itself a non-word char) yet stays one word.
        assert_extracts("ज्ञानकोश है", &["ज्ञानकोश"]);
        // Hangul syllables are word characters.
        assert_extracts("한국어 위키백과", &["한국어", "위키백과"]);
        // An emoji cluster is not.
        assert_extracts("emoji 👍🏽 word", &["emoji", "word"]);
    }

    /// Every word extracted from the corpora must satisfy the documented invariants.
    #[test]
    fn extract_corpora_invariants() {
        for (name, text) in CORPORA {
            let rope = Rope::from_str(text);
            let mut count = 0;
            for word in words(rope.slice(..)) {
                count += 1;
                let clusters = word.graphemes().count();
                assert!(
                    clusters >= MIN_WORD_GRAPHEMES,
                    "{name}: {word:?} spans only {clusters} grapheme cluster(s)"
                );
                assert!(
                    word.len_chars() <= MAX_WORD_LEN,
                    "{name}: {word:?} is {} chars long",
                    word.len_chars()
                );
                assert!(
                    word.chars().next().is_some_and(char_is_word),
                    "{name}: {word:?} starts with a non-word character"
                );
            }
            assert!(count > 0, "{name}: expected to extract some words");
        }
    }

    /// Grapheme-cluster edge cases which the prose corpora underrepresent: combining marks at
    /// word starts and seams, a skin-tone emoji, a ZWJ family sequence, a Devanagari conjunct,
    /// and a lone Hangul syllable.
    const SPICE: &[&str] = &[
        "a\u{0301}",              // base + combining acute
        "\u{0301}lead",           // leading combining mark
        "mid\u{0308}dle",         // combining diaeresis mid-word
        "👍🏽",                     // emoji + skin-tone modifier
        "👨\u{200D}👩\u{200D}👧", // ZWJ family sequence
        "क्ष",                     // Devanagari conjunct (virama)
        "한",                     // Hangul syllable
    ];

    /// Picks a chunk of text: usually a random char span sliced out of one of the multilingual
    /// corpora (real grapheme complexity for free, and slicing on char boundaries deliberately
    /// frays some clusters), occasionally an adversarial cluster from [`SPICE`].
    fn sample_text(g: &mut Gen) -> String {
        if usize::arbitrary(g) % 4 == 0 {
            return g.choose(SPICE).unwrap().to_string();
        }
        let (_, corpus) = g.choose(CORPORA).unwrap();
        let chars: Vec<char> = corpus.chars().collect();
        if chars.is_empty() {
            return String::new();
        }
        let span = usize::arbitrary(g) % 200;
        let start = usize::arbitrary(g) % chars.len();
        let end = (start + span).min(chars.len());
        chars[start..end].iter().collect()
    }

    /// Applies one random splice to `text`: delete a random char range and insert a fresh chunk.
    fn random_edit(g: &mut Gen, text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let del_start = usize::arbitrary(g) % (len + 1);
        let del_len = usize::arbitrary(g) % (len - del_start + 1);
        let mut out: String = chars[..del_start].iter().collect();
        out.push_str(&sample_text(g));
        out.extend(&chars[del_start + del_len..]);
        out
    }

    /// An obvious, independent reimplementation of [`words`]: walk grapheme clusters, accumulate
    /// a run of consecutive word-character clusters, and emit it when it ends if it satisfies the
    /// length bounds. Used as an oracle for the optimized single-pass extractor.
    fn reference_words(text: &str) -> Vec<String> {
        let rope = Rope::from_str(text);
        let mut out = Vec::new();
        let mut run = String::new();
        let mut clusters = 0usize;
        let flush = |run: &mut String, clusters: &mut usize, out: &mut Vec<String>| {
            if *clusters >= MIN_WORD_GRAPHEMES && run.chars().count() <= MAX_WORD_LEN {
                out.push(run.clone());
            }
            run.clear();
            *clusters = 0;
        };
        for cluster in rope.slice(..).graphemes() {
            if cluster.chars().next().is_some_and(char_is_word) {
                run.extend(cluster.chars());
                clusters += 1;
            } else {
                flush(&mut run, &mut clusters, &mut out);
            }
        }
        flush(&mut run, &mut clusters, &mut out);
        out
    }

    #[derive(Clone, Debug)]
    struct SampledText(String);

    impl Arbitrary for SampledText {
        fn arbitrary(g: &mut Gen) -> Self {
            SampledText(sample_text(g))
        }
        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            Box::new(self.0.shrink().map(SampledText))
        }
    }

    #[derive(Clone, Debug)]
    struct EditPair {
        old: String,
        new: String,
    }

    impl Arbitrary for EditPair {
        fn arbitrary(g: &mut Gen) -> Self {
            let old = sample_text(g);
            let new = random_edit(g, &old);
            EditPair { old, new }
        }
        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            Box::new(
                (self.old.clone(), self.new.clone())
                    .shrink()
                    .map(|(old, new)| EditPair { old, new }),
            )
        }
    }

    quickcheck::quickcheck! {
        /// The optimized single-pass extractor agrees with the obvious reference on arbitrary
        /// (multilingual, cluster-frayed) text.
        fn prop_words_match_reference(text: SampledText) -> bool {
            let rope = Rope::from_str(&text.0);
            let got: Vec<String> = words(rope.slice(..)).map(|w| w.to_string()).collect();
            got == reference_words(&text.0)
        }

        /// Incrementally updating across one edit lands on exactly the same word multiset (words
        /// and refcounts) as reindexing the new text from scratch.
        fn prop_snapshot_updates_match_fresh(pair: EditPair) -> bool {
            let old = Rope::from_str(&pair.old);
            let new = Rope::from_str(&pair.new);

            let mut incremental = WordIndex::default();
            incremental.add_document(&old, || false);
            incremental.update(&old, &new, || false);

            let mut fresh = WordIndex::default();
            fresh.add_document(&new, || false);

            incremental.counts() == fresh.counts()
        }

    }
}

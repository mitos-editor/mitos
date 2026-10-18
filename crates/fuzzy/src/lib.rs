//! Fuzzy matching policies and reusable scoring buffers.
//!
//! Completion sessions, pickers, and word indexes own their matchers. Small
//! synchronous lists can use [`fuzzy_match`], which caches a matcher per thread.

use nucleo_matcher::{
    pattern::{Atom, AtomKind, CaseMatching, Normalization, Pattern},
    Config, Utf32Str,
};
use std::{cell::Cell, fmt};

/// Matching policy shared by scoring and highlight indices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MatchKind {
    #[default]
    Text,
    Path,
    Completion,
}

impl MatchKind {
    fn config(self) -> Config {
        let mut config = Config::DEFAULT;
        match self {
            Self::Text => {}
            Self::Path => config.set_match_paths(),
            Self::Completion => config.prefer_prefix = true,
        }
        config
    }
}

/// A compiled fuzzy query, independent of the matcher's scratch buffers.
pub struct Query(Atom);

impl Query {
    /// Length in matcher characters, used by completion's ranking threshold.
    pub fn needle_len(&self) -> usize {
        self.0.needle_text().len()
    }
}

/// Reusable scoring and Unicode buffers, allocated on first use.
#[derive(Default)]
pub struct Matcher {
    kind: MatchKind,
    inner: Option<nucleo_matcher::Matcher>,
    scratch: Vec<char>,
}

impl fmt::Debug for Matcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Matcher")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Matcher {
    pub fn new(kind: MatchKind) -> Self {
        Self {
            kind,
            ..Self::default()
        }
    }

    pub fn set_kind(&mut self, kind: MatchKind) {
        self.kind = kind;
        if let Some(inner) = &mut self.inner {
            inner.config = kind.config();
        }
    }

    pub fn query(&self, pattern: &str) -> Query {
        let case = if self.kind == MatchKind::Completion {
            CaseMatching::Ignore
        } else {
            CaseMatching::Smart
        };
        Query(Atom::new(
            pattern,
            case,
            Normalization::Smart,
            AtomKind::Fuzzy,
            false,
        ))
    }

    fn engine(&mut self) -> &mut nucleo_matcher::Matcher {
        self.inner
            .get_or_insert_with(|| nucleo_matcher::Matcher::new(self.kind.config()))
    }

    pub fn score(&mut self, query: &Query, text: &str) -> Option<u16> {
        let inner = self
            .inner
            .get_or_insert_with(|| nucleo_matcher::Matcher::new(self.kind.config()));
        query.0.score(Utf32Str::new(text, &mut self.scratch), inner)
    }

    pub fn match_list<T: AsRef<str>>(
        &mut self,
        pattern: &str,
        items: impl IntoIterator<Item = T>,
    ) -> Vec<(T, u16)> {
        let query = self.query(pattern);
        query.0.match_list(items, self.engine())
    }

    /// Append highlight indices for a query from an asynchronous Nucleo picker.
    pub fn indices(
        &mut self,
        pattern: &Pattern,
        text: Utf32Str<'_>,
        indices: &mut Vec<u32>,
    ) -> Option<u32> {
        pattern.indices(text, self.engine(), indices)
    }
}

thread_local! {
    static CACHED: Cell<Option<Matcher>> = const { Cell::new(None) };
}

/// Match a small list synchronously, preserving Nucleo's score ordering.
/// Large lists should use a background matching engine.
pub fn fuzzy_match<T: AsRef<str>>(
    pattern: &str,
    items: impl IntoIterator<Item = T>,
    kind: MatchKind,
) -> Vec<(T, u16)> {
    // Take ownership so nested matching does not retain a thread-local borrow.
    let mut matcher = CACHED.with(Cell::take).unwrap_or_default();
    matcher.set_kind(kind);
    let matches = matcher.match_list(pattern, items);
    CACHED.with(|cache| cache.set(Some(matcher)));
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_uses_smart_case_and_completion_ignores_case() {
        let mut text = Matcher::new(MatchKind::Text);
        assert_eq!(
            text.match_list("AB", ["ab", "AB"])
                .into_iter()
                .map(|(text, _)| text)
                .collect::<Vec<_>>(),
            ["AB"]
        );
        let mut completion = Matcher::new(MatchKind::Completion);
        assert_eq!(completion.match_list("AB", ["ab", "AB"]).len(), 2);
        let query = completion.query("ab");
        assert!(completion.score(&query, "ab_suffix") > completion.score(&query, "prefix_ab"));
    }

    #[test]
    fn highlight_indices_follow_unicode_matcher_characters() {
        let mut matcher = Matcher::new(MatchKind::Text);
        let pattern = Pattern::parse("éw", CaseMatching::Smart, Normalization::Smart);
        let mut buffer = Vec::new();
        let text = Utf32Str::new("é word", &mut buffer);
        let mut indices = Vec::new();
        assert!(matcher.indices(&pattern, text, &mut indices).is_some());
        assert_eq!(indices, [0, 2]);
    }
}

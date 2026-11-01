//! Workspace text search independent of editor state, runtime paths, and UI.
//!
//! Callers provide a root, traversal options, unsaved text snapshots, cancellation,
//! and a result sink. Matching runs synchronously with parallel directory walking;
//! applications choose the worker on which to run it.

use editor_core::{Rope, RopeReader};
use grep_matcher::{Captures, Matcher};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, SearcherBuilder, Sink, SinkMatch};
use ignore::WalkState;

mod file_index;
mod walk;
pub use file_index::FileIndex;
use std::{
    collections::HashMap,
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
pub use walk::{entry_file_type, include_entry, Options};

/// Zero-based line and character-column bounds for one matching line or block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub path: PathBuf,
    pub line_start: usize,
    pub line_end: usize,
    pub match_start_col: usize,
    pub match_end_col: usize,
}

pub struct Search {
    matcher: RegexMatcher,
}

impl Search {
    pub fn new(pattern: &str, smart_case: bool) -> Result<Self, grep_regex::Error> {
        Ok(Self {
            matcher: RegexMatcherBuilder::new()
                .case_smart(smart_case)
                .multi_line(true)
                .build(pattern)?,
        })
    }

    /// Expand captures into character-indexed edits, optionally for one exact match.
    pub fn replacement_changes(
        &self,
        text: editor_core::RopeSlice<'_>,
        replacement: &str,
        target: Option<std::ops::Range<usize>>,
    ) -> anyhow::Result<Vec<(usize, usize, Option<editor_core::Tendril>)>> {
        let haystack = text.to_string();
        let mut captures = self.matcher.new_captures()?;
        let mut changes = Vec::new();
        let mut invalid_boundary = false;
        // Keep canonical non-overlapping capture iteration, but reject other
        // matches before paying for two rope position conversions per match.
        let target_bytes = target
            .as_ref()
            .filter(|range| range.start <= range.end && range.end <= text.len_chars())
            .map(|range| text.char_to_byte_idx(range.start)..text.char_to_byte_idx(range.end));
        self.matcher
            .captures_iter(haystack.as_bytes(), &mut captures, |captures| {
                let matched = captures.get(0).unwrap();
                if !haystack.is_char_boundary(matched.start())
                    || !haystack.is_char_boundary(matched.end())
                {
                    // The byte regex engine can report empty matches inside a scalar.
                    // Skip those; nonempty partial scalars cannot become text edits.
                    invalid_boundary |= matched.start() != matched.end();
                    return !invalid_boundary;
                }
                let (start, end) = if let Some(target) = &target {
                    if target_bytes.as_ref().is_none_or(|range| {
                        matched.start() != range.start || matched.end() != range.end
                    }) {
                        return true;
                    }
                    (target.start, target.end)
                } else {
                    (
                        text.byte_to_char_idx(matched.start()),
                        text.byte_to_char_idx(matched.end()),
                    )
                };
                let mut expanded = Vec::new();
                captures.interpolate(
                    |name| self.matcher.capture_index(name),
                    haystack.as_bytes(),
                    replacement.as_bytes(),
                    &mut expanded,
                );
                changes.push((start, end, expanded));
                target.is_none()
            })?;
        anyhow::ensure!(
            !invalid_boundary,
            "search pattern matches inside a UTF-8 character"
        );
        changes
            .into_iter()
            .map(|(start, end, expanded)| {
                Ok((start, end, Some(String::from_utf8(expanded)?.into())))
            })
            .collect()
    }

    /// Search an edited buffer using the same line/block semantics as workspace search.
    /// This does not walk directories or read files from disk.
    pub fn search_document(&self, path: &Path, text: &Rope) -> std::io::Result<Vec<Match>> {
        let mut results = Vec::new();
        search_file(
            &mut searcher(),
            &self.matcher,
            path,
            Some(text),
            || false,
            |matched| {
                results.push(matched);
                ControlFlow::Continue(())
            },
        )?;
        Ok(results)
    }

    /// Stream results until traversal finishes, cancellation is requested, or the sink stops.
    /// Snapshots take precedence at the walked path or its resolved filesystem path. Unreadable entries are skipped
    /// and search I/O errors are logged, allowing other files to produce results.
    pub fn run(
        &self,
        root: &Path,
        options: &Options,
        documents: &HashMap<PathBuf, Rope>,
        is_cancelled: impl Fn() -> bool + Sync,
        emit: impl Fn(Match) -> ControlFlow<()> + Sync,
    ) {
        let stopped = AtomicBool::new(false);
        let cancelled = || stopped.load(Ordering::Relaxed) || is_cancelled();
        let searcher = searcher();
        let walker = options.walker(root);
        walker.build_parallel().run(|| {
            let mut searcher = searcher.clone();
            let matcher = self.matcher.clone();
            let cancelled = &cancelled;
            let emit = &emit;
            let stopped = &stopped;
            Box::new(move |entry| {
                if cancelled() {
                    return WalkState::Quit;
                }
                let entry = match entry {
                    Ok(entry) if entry_file_type(&entry).is_some_and(|kind| kind.is_file()) => {
                        entry
                    }
                    _ => return WalkState::Continue,
                };
                let snapshot = documents.get(entry.path()).or_else(|| {
                    if documents.is_empty() {
                        return None;
                    }
                    documents.get(&entry.path().canonicalize().ok()?)
                });
                let result = search_file(
                    &mut searcher,
                    &matcher,
                    entry.path(),
                    snapshot,
                    cancelled,
                    |matched| {
                        let flow = emit(matched);
                        if flow.is_break() {
                            stopped.store(true, Ordering::Relaxed);
                        }
                        flow
                    },
                );
                if let Err(error) = result {
                    log::error!("Global search error: {}, {}", entry.path().display(), error);
                }
                if cancelled() {
                    WalkState::Quit
                } else {
                    WalkState::Continue
                }
            })
        });
    }
}

fn searcher() -> grep_searcher::Searcher {
    SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .multi_line(true)
        .build()
}

fn search_file(
    searcher: &mut grep_searcher::Searcher,
    matcher: &RegexMatcher,
    path: &Path,
    text: Option<&Rope>,
    cancelled: impl Fn() -> bool,
    mut emit: impl FnMut(Match) -> ControlFlow<()>,
) -> std::io::Result<()> {
    let sink = MatchSink(|matched: &SinkMatch<'_>| {
        if cancelled() {
            return Ok(false);
        }
        // Keep the original haystack around the match. Slicing a matching line
        // first would give document anchors such as \A and \z a new meaning.
        let range = matched.bytes_range_in_buffer();
        let Some(found) = matcher.find_at(matched.buffer(), range.start).unwrap() else {
            return Ok(true);
        };
        if found.start() > range.end
            || found.end() > range.end
            || (found.start() == range.end && !matched.bytes().is_empty())
        {
            return Ok(true);
        }
        let line_content = std::str::from_utf8(matched.bytes())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let Some((line_start, match_start_col, line_end, match_end_col)) = match_line_cols(
            matched.line_number().unwrap() as usize - 1,
            line_content,
            found.start() - range.start..found.end() - range.start,
        ) else {
            return Ok(true);
        };
        let flow = emit(Match {
            path: path.to_owned(),
            line_start,
            line_end,
            match_start_col,
            match_end_col,
        });
        Ok(flow.is_continue() && !cancelled())
    });
    if let Some(text) = text {
        if searcher.multi_line_with_matcher(matcher) {
            searcher.search_slice(matcher, text.to_string().as_bytes(), sink)
        } else {
            searcher.search_reader(matcher, RopeReader::new(text.slice(..)), sink)
        }
    } else {
        searcher.search_path(matcher, path, sink)
    }
}

struct MatchSink<F>(F);

impl<F: FnMut(&SinkMatch<'_>) -> std::io::Result<bool>> Sink for MatchSink<F> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _: &grep_searcher::Searcher,
        matched: &SinkMatch<'_>,
    ) -> std::io::Result<bool> {
        (self.0)(matched)
    }
}

/// Converts the regex engine's byte match within `line_content` into
/// zero-based line and character-column bounds relative to `line_start`.
fn match_line_cols(
    line_start: usize,
    line_content: &str,
    matched: std::ops::Range<usize>,
) -> Option<(usize, usize, usize, usize)> {
    let prefix = line_content.get(..matched.start)?;
    let matched_text = line_content.get(..matched.end)?;

    let start_line = line_start + prefix.matches('\n').count();
    let start_col = prefix.rsplit('\n').next().unwrap_or(prefix).chars().count();

    let end_line = line_start + matched_text.matches('\n').count();
    let end_col = matched_text
        .rsplit('\n')
        .next()
        .unwrap_or(matched_text)
        .chars()
        .count();

    Some((start_line, start_col, end_line, end_col))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicUsize, Mutex};

    #[test]
    fn selected_replacement_keeps_canonical_matches_and_validates_targets() {
        for (pattern, text, target) in [
            ("aa", "aaa", 1..3),
            ("a*", "a", 1..1),
            ("foo", "foo", 0..4),
            ("foo", "foo", std::ops::Range { start: 3, end: 2 }),
            ("foo", "foo", usize::MAX..usize::MAX),
        ] {
            assert!(Search::new(pattern, false)
                .unwrap()
                .replacement_changes(Rope::from_str(text).slice(..), "x", Some(target))
                .unwrap()
                .is_empty());
        }
        let text = Rope::from_str("é foo\n😀 foo\n");
        let search = Search::new(r"(?m)^(😀) (foo)$", false).unwrap();
        assert_eq!(
            search
                .replacement_changes(text.slice(..), "$2:$1", Some(6..11))
                .unwrap(),
            vec![(6, 11, Some("foo:😀".into()))]
        );
        assert!(Search::new(r"(?-u:\xA9)", false)
            .unwrap()
            .replacement_changes(Rope::from_str("é").slice(..), "x", Some(5..6))
            .is_err());
        assert_eq!(
            Search::new("", false)
                .unwrap()
                .replacement_changes(Rope::from_str("é😀").slice(..), "x", Some(1..1))
                .unwrap(),
            vec![(1, 1, Some("x".into()))]
        );
    }

    #[test]
    fn replacement_expands_captures_for_all_matches() {
        let text = Rope::from("foo-12 FOO-34\n");
        let search = Search::new(r"(?P<word>foo)-(\d+)", true).unwrap();
        let changes = search
            .replacement_changes(text.slice(..), "${word}:$2 $$", None)
            .unwrap();
        assert_eq!(
            changes,
            vec![
                (0, 6, Some("foo:12 $".into())),
                (7, 13, Some("FOO:34 $".into())),
            ]
        );
    }

    #[test]
    fn replacement_targets_character_ranges_and_rejects_stale_matches() {
        let text = Rope::from("α foo foo\n");
        let search = Search::new("foo", true).unwrap();
        assert_eq!(
            search
                .replacement_changes(text.slice(..), "bar", Some(6..9))
                .unwrap(),
            vec![(6, 9, Some("bar".into()))]
        );
        assert!(search
            .replacement_changes(text.slice(..), "bar", Some(5..8))
            .unwrap()
            .is_empty());
        assert_eq!(
            Search::new("foo", false)
                .unwrap()
                .replacement_changes(Rope::from("FOO foo").slice(..), "", None)
                .unwrap(),
            vec![(4, 7, Some("".into()))]
        );
    }

    #[test]
    fn replacement_handles_multiline_and_empty_matches() {
        let text = Rope::from("α\nβ\n");
        assert_eq!(
            Search::new("α\\nβ", true)
                .unwrap()
                .replacement_changes(text.slice(..), "x", None)
                .unwrap(),
            vec![(0, 3, Some("x".into()))]
        );
        let changes = Search::new("", true)
            .unwrap()
            .replacement_changes(Rope::from("αβ").slice(..), "x", None)
            .unwrap();
        assert_eq!(
            changes,
            vec![
                (0, 0, Some("x".into())),
                (1, 1, Some("x".into())),
                (2, 2, Some("x".into()))
            ]
        );
    }

    #[test]
    fn replacement_rejects_partial_utf8_matches_and_captures() {
        let text = Rope::from("é");
        for (pattern, replacement) in [(r"(?-u:\xA9)", "x"), (r"(?-u:(.)(.))", "$1")] {
            assert!(Search::new(pattern, true)
                .unwrap()
                .replacement_changes(text.slice(..), replacement, None)
                .is_err());
        }
        assert!(match_line_cols(0, "é", 1..2).is_none());
    }

    fn options() -> Options {
        Options {
            parents: false,
            git_global: false,
            ..Options::default()
        }
    }

    fn collect(
        root: &Path,
        options: &Options,
        documents: &HashMap<PathBuf, Rope>,
        pattern: &str,
    ) -> Vec<Match> {
        let results = Mutex::new(Vec::new());
        Search::new(pattern, true).unwrap().run(
            root,
            options,
            documents,
            || false,
            |matched| {
                results.lock().unwrap().push(matched);
                ControlFlow::Continue(())
            },
        );
        results.into_inner().unwrap()
    }

    #[test]
    fn unsaved_snapshots_override_disk_and_use_character_columns_across_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document.txt");
        std::fs::write(&path, "disk contents\n").unwrap();
        let documents = HashMap::from([(path.clone(), Rope::from("x α\nβ z\n"))]);
        assert_eq!(
            collect(dir.path(), &options(), &documents, "α\\nβ"),
            [Match {
                path,
                line_start: 0,
                match_start_col: 2,
                line_end: 1,
                match_end_col: 1,
            }]
        );
        assert!(collect(dir.path(), &options(), &documents, "disk").is_empty());
    }

    #[test]
    fn document_search_has_the_same_results_as_workspace_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document.txt");
        std::fs::write(&path, "unrelated disk contents").unwrap();
        let text = Rope::from("α foo foo\r\nβ FOO\nfoo\n");
        let documents = HashMap::from([(path.clone(), text.clone())]);
        for pattern in ["foo", "foo\\r?\\nβ", "(?m)^", "(?i)foo"] {
            assert_eq!(
                Search::new(pattern, true)
                    .unwrap()
                    .search_document(&path, &text)
                    .unwrap(),
                collect(dir.path(), &options(), &documents, pattern),
                "{pattern}",
            );
        }
        // The direct buffer path also works when the file no longer exists on disk.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            Search::new("foo", true)
                .unwrap()
                .search_document(&path, &text)
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn traversal_respects_ignore_files_binary_detection_and_vcs_directories() {
        let dir = tempfile::tempdir().unwrap();
        for (name, contents) in [
            ("visible", "needle"),
            ("ignored", "needle"),
            ("custom", "needle"),
            ("binary", "\0needle"),
        ] {
            std::fs::write(dir.path().join(name), contents).unwrap();
        }
        std::fs::write(dir.path().join(".ignore"), "ignored\n").unwrap();
        std::fs::write(dir.path().join("search.ignore"), "custom\n").unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/noise"), "needle").unwrap();
        let options = Options {
            hidden: false,
            custom_ignore_files: vec!["search.ignore".into()],
            ..options()
        };
        let results = collect(dir.path(), &options, &HashMap::new(), "needle");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, dir.path().join("visible"));
    }

    #[test]
    fn cancellation_is_observed_without_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..20 {
            std::fs::write(dir.path().join(i.to_string()), "no match").unwrap();
        }
        let checks = AtomicUsize::new(0);
        Search::new("needle", true).unwrap().run(
            dir.path(),
            &options(),
            &HashMap::new(),
            || checks.fetch_add(1, Ordering::Relaxed) > 0,
            |_| panic!("no file matches"),
        );
        assert!(checks.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn sink_can_stop_before_the_remaining_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("matches"), "needle\n".repeat(100)).unwrap();
        let results = AtomicUsize::new(0);
        Search::new("needle", true).unwrap().run(
            dir.path(),
            &options(),
            &HashMap::new(),
            || false,
            |_| {
                results.fetch_add(1, Ordering::Relaxed);
                ControlFlow::Break(())
            },
        );
        assert_eq!(results.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn search_and_replacement_agree_on_document_anchors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        for prefix in ["start\n".into(), format!("{}\n", "x".repeat(100_000))] {
            let text = Rope::from(format!("{prefix}foo bar\n"));
            std::fs::write(&path, text.to_string()).unwrap();
            let search = Search::new(r"\Afoo|bar", true).unwrap();
            let rows = search.search_document(&path, &text).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].match_start_col, 4, "only bar matches on this line");
            assert_eq!(
                collect(dir.path(), &options(), &HashMap::new(), r"\Afoo|bar"),
                rows
            );
            let start = prefix.chars().count() + 4;
            assert_eq!(
                search
                    .replacement_changes(text.slice(..), "x", Some(start..start + 3))
                    .unwrap(),
                vec![(start, start + 3, Some("x".into()))]
            );
        }
    }

    #[test]
    fn reported_ranges_are_replaceable_with_the_original_haystack() {
        for text in ["", "α\nβ\n", "foo\r\n", "start\nfoo bar\n"] {
            for pattern in [
                r"\A",
                r"\z",
                "(?m)^",
                "(?m)$",
                "foo",
                r"foo\r?\n",
                r"\Afoo|bar",
            ] {
                let search = Search::new(pattern, true).unwrap();
                let rope = Rope::from(text);
                for matched in search.search_document(Path::new("file"), &rope).unwrap() {
                    let offset = |line| {
                        text.split_inclusive('\n')
                            .take(line)
                            .map(|line| line.chars().count())
                            .sum::<usize>()
                    };
                    let range = offset(matched.line_start) + matched.match_start_col
                        ..offset(matched.line_end) + matched.match_end_col;
                    assert_eq!(
                        search
                            .replacement_changes(rope.slice(..), "x", Some(range))
                            .unwrap()
                            .len(),
                        1,
                        "pattern {pattern:?}, text {text:?}, match {matched:?}"
                    );
                }
            }
        }
    }
}

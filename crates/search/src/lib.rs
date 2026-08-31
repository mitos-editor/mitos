//! Workspace text search independent of editor state, runtime paths, and UI.
//!
//! Callers provide a root, traversal options, unsaved text snapshots, cancellation,
//! and a result sink. Matching runs synchronously with parallel directory walking;
//! applications choose the worker on which to run it.

use editor_core::{Rope, RopeReader};
use grep_matcher::{Captures, Matcher};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{sinks, BinaryDetection, SearcherBuilder};
use ignore::WalkState;

mod walk;
use std::{
    collections::HashMap,
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
pub use walk::{include_entry, Options};

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
                let start = text.byte_to_char_idx(matched.start());
                let end = text.byte_to_char_idx(matched.end());
                if target
                    .as_ref()
                    .is_some_and(|target| *target != (start..end))
                {
                    return true;
                }
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

    /// Stream results until traversal finishes, cancellation is requested, or the sink stops.
    /// Snapshots take precedence over files at the same path. Unreadable entries are skipped
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
        let searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .multi_line(true)
            .build();
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
                    Ok(entry) if entry.path().is_file() => entry,
                    _ => return WalkState::Continue,
                };
                let sink = sinks::UTF8(|line_start, line_content| {
                    if cancelled() {
                        return Ok(false);
                    }
                    let Some((line_start, match_start_col, line_end, match_end_col)) =
                        match_line_cols(line_start as usize - 1, line_content, &matcher)
                    else {
                        return Ok(true);
                    };
                    if emit(Match {
                        path: entry.path().to_owned(),
                        line_start,
                        line_end,
                        match_start_col,
                        match_end_col,
                    })
                    .is_break()
                    {
                        stopped.store(true, Ordering::Relaxed);
                    }
                    Ok(!cancelled())
                });
                let result = if let Some(text) = documents.get(entry.path()) {
                    if searcher.multi_line_with_matcher(&matcher) {
                        searcher.search_slice(&matcher, text.to_string().as_bytes(), sink)
                    } else {
                        searcher.search_reader(&matcher, RopeReader::new(text.slice(..)), sink)
                    }
                } else {
                    searcher.search_path(&matcher, entry.path(), sink)
                };
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

/// Converts the regex engine's byte match within `line_content` into
/// zero-based line and character-column bounds relative to `line_start`.
fn match_line_cols(
    line_start: usize,
    line_content: &str,
    matcher: &grep_regex::RegexMatcher,
) -> Option<(usize, usize, usize, usize)> {
    let matched = matcher.find(line_content.as_bytes()).ok().flatten()?;
    let prefix = line_content.get(..matched.start())?;
    let matched_text = line_content.get(..matched.end())?;

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
        assert!(
            match_line_cols(0, "é", &Search::new(r"(?-u:\xA9)", true).unwrap().matcher).is_none()
        );
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
}

//! Workspace text search independent of editor state, runtime paths, and UI.
//!
//! Callers provide a root, traversal options, unsaved text snapshots, cancellation,
//! and a result sink. Matching runs synchronously with parallel directory walking;
//! applications choose the worker on which to run it.

use editor_core::{Rope, RopeReader};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{sinks, BinaryDetection, SearcherBuilder};
use ignore::{DirEntry, WalkBuilder, WalkState};
use std::{
    collections::HashMap,
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// Zero-based line and character-column bounds for one matching line or block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub path: PathBuf,
    pub line_start: usize,
    pub line_end: usize,
    pub match_start_col: usize,
    pub match_end_col: usize,
}

/// Directory traversal policy, supplied independently of application settings.
#[derive(Debug, Clone)]
pub struct Options {
    pub hidden: bool,
    pub parents: bool,
    pub ignore: bool,
    pub follow_symlinks: bool,
    pub deduplicate_links: bool,
    pub git_ignore: bool,
    pub git_global: bool,
    pub git_exclude: bool,
    pub max_depth: Option<usize>,
    pub custom_ignore_files: Vec<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            hidden: true,
            parents: true,
            ignore: true,
            follow_symlinks: true,
            deduplicate_links: true,
            git_ignore: true,
            git_global: true,
            git_exclude: true,
            max_depth: None,
            custom_ignore_files: Vec::new(),
        }
    }
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
        let absolute_root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
        let deduplicate_links = options.deduplicate_links;
        let stopped = AtomicBool::new(false);
        let cancelled = || stopped.load(Ordering::Relaxed) || is_cancelled();
        let searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .multi_line(true)
            .build();
        let mut walker = WalkBuilder::new(root);
        walker
            .hidden(options.hidden)
            .parents(options.parents)
            .ignore(options.ignore)
            .follow_links(options.follow_symlinks)
            .git_ignore(options.git_ignore)
            .git_global(options.git_global)
            .git_exclude(options.git_exclude)
            .max_depth(options.max_depth)
            .filter_entry(move |entry| include_entry(entry, &absolute_root, deduplicate_links));
        for filename in &options.custom_ignore_files {
            walker.add_custom_ignore_filename(filename);
        }
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
    let prefix = &line_content[..matched.start()];
    let matched_text = &line_content[..matched.end()];

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

/// Shared traversal policy for search and file/symbol pickers.
/// Excludes VCS internals and optionally symlinks into the search root.
pub fn include_entry(entry: &DirEntry, root: &Path, dedup_symlinks: bool) -> bool {
    // We always want to ignore popular VCS directories, otherwise if
    // `ignore` is turned off, we end up with a lot of noise
    // in our picker.
    if matches!(
        entry.file_name().to_str(),
        Some(".git" | ".pijul" | ".jj" | ".hg" | ".svn")
    ) {
        return false;
    }

    // We also ignore symlinks that point inside the current directory
    // if `dedup_links` is enabled.
    if dedup_symlinks && entry.path_is_symlink() {
        return entry
            .path()
            .canonicalize()
            .ok()
            .is_some_and(|path| !path.starts_with(root));
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicUsize, Mutex};

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

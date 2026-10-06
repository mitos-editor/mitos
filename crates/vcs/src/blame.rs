use std::{cell::LazyCell, ops::Range, path::PathBuf, sync::Arc};

#[cfg(feature = "git")]
use std::collections::HashMap;

use anyhow::Result;

/// Blame for the committed version of a file, with commit metadata prepared off-thread.
#[derive(Debug)]
pub struct FileBlame {
    #[cfg(feature = "git")]
    source: BlameSource,
    ranges: Vec<BlameRange>,
    commits: Vec<LineBlame>,
}

#[cfg(feature = "git")]
#[derive(Debug, PartialEq, Eq)]
struct BlameSource {
    path: PathBuf,
    revision: gix::ObjectId,
    trust_full: bool,
}

#[derive(Debug)]
struct BlameRange {
    lines: Range<u32>,
    commit: usize,
}

impl FileBlame {
    /// Borrow metadata for a zero-based line in the committed file.
    pub fn blame_for_line(&self, line: u32) -> Option<&LineBlame> {
        let index = self.ranges.partition_point(|range| range.lines.end <= line);
        let range = self.ranges.get(index)?;
        range
            .lines
            .contains(&line)
            .then(|| &self.commits[range.commit])
    }

    /// Compute blame using the caller's workspace Git trust decision.
    pub fn try_new(file: PathBuf, trust_full: bool) -> Result<Arc<Self>> {
        Self::try_refresh(file, trust_full, None)
    }

    /// Reuse a snapshot only when its canonical path, HEAD commit and trust match.
    #[cfg(not(feature = "git"))]
    pub fn try_refresh(
        _file: PathBuf,
        _trust_full: bool,
        _cached: Option<Arc<Self>>,
    ) -> Result<Arc<Self>> {
        anyhow::bail!("Git blame support is not compiled in")
    }

    #[cfg(feature = "git")]
    pub fn try_refresh(
        file: PathBuf,
        trust_full: bool,
        cached: Option<Arc<Self>>,
    ) -> Result<Arc<Self>> {
        use crate::git::{get_repo_dir, open_repo};
        use anyhow::Context as _;

        let file = gix::path::realpath(&file).context("resolve symlinks")?;
        let file = stdx::path::canonicalize_existing(&file);
        let repo = open_repo(get_repo_dir(&file)?, trust_full)
            .context("Failed to open git repo")?
            .to_thread_local();
        let head = repo.head_commit()?.id;
        let source = BlameSource {
            path: file.clone(),
            revision: head,
            trust_full,
        };
        if let Some(cached) = cached
            && cached.source == source
        {
            return Ok(cached);
        }
        let workdir = repo
            .workdir()
            .context("Git blame requires a working tree")?;
        // Git's linked-worktree metadata can omit the Windows verbatim prefix.
        // Canonicalize both sides before comparing paths, allowing a missing file.
        let workdir = stdx::path::canonicalize_existing(workdir);
        // Use the working tree root, including linked worktrees and subdirectories.
        let path = gix::path::to_unix_separators_on_windows(gix::path::try_into_bstr(
            file.strip_prefix(&workdir)?,
        )?);
        let entries = repo
            .blame_file(path.as_ref(), head, Default::default())?
            .entries;
        let mut commit_indices = HashMap::new();
        let mut commits = Vec::new();
        let mut ranges = Vec::with_capacity(entries.len());
        for entry in entries {
            let commit_index = if let Some(&index) = commit_indices.get(&entry.commit_id) {
                index
            } else {
                let commit = repo.find_commit(entry.commit_id)?;
                let message = commit.message().ok();
                let author = commit.author().ok();
                let time = author.and_then(|author| author.time.parse::<gix::date::Time>().ok());
                let line_blame = LineBlame {
                    commit_hash: commit.short_id().ok().map(|id| id.to_string()),
                    author_name: author.map(|author| author.name.to_string()),
                    author_email: author.map(|author| author.email.to_string()),
                    commit_date: time
                        .and_then(|time| time.format(gix::date::time::format::SHORT).ok()),
                    commit_title: message
                        .as_ref()
                        .map(|message| message.title.to_string().trim_end().to_owned()),
                    commit_body: message
                        .as_ref()
                        .and_then(|message| message.body.map(|body| body.to_string())),
                    time_stamp: time.map(|time| (time.seconds, time.offset)),
                };
                let index = commits.len();
                commits.push(line_blame);
                commit_indices.insert(entry.commit_id, index);
                index
            };
            ranges.push(BlameRange {
                lines: entry.start_in_blamed_file..entry.start_in_blamed_file + entry.len.get(),
                commit: commit_index,
            });
        }
        ranges.sort_unstable_by_key(|range| range.lines.start);
        Ok(Arc::new(Self {
            source,
            ranges,
            commits,
        }))
    }
}

#[derive(Default, Debug)]
pub struct LineBlame {
    commit_hash: Option<String>,
    author_name: Option<String>,
    author_email: Option<String>,
    commit_date: Option<String>,
    commit_title: Option<String>,
    commit_body: Option<String>,
    /// Used to compute `time-ago`
    time_stamp: Option<(i64, i32)>,
}

impl LineBlame {
    /// # Returns
    ///
    /// None => Invalid variable
    /// Some(None) => Valid variable, but is empty
    #[inline]
    fn get_variable(&self, var: &str) -> Option<Option<&str>> {
        Some(
            match var {
                "commit" => &self.commit_hash,
                "author" => &self.author_name,
                "date" => &self.commit_date,
                "title" => &self.commit_title,
                "email" => &self.author_email,
                "body" => &self.commit_body,
                _ => return None,
            }
            .as_deref(),
        )
    }

    /// Format borrowed commit metadata; compute relative time only if requested.
    pub fn parse_format(&self, format: &str) -> String {
        let time_ago = LazyCell::new(|| {
            self.time_stamp
                .map(|(seconds, offset)| stdx::time::format_relative_time(seconds, offset))
        });
        let mut output = String::with_capacity(format.len());
        let mut literal_start = 0;
        let mut exclude_next_literal = false;
        let mut chars = format.char_indices().peekable();
        while let Some((start, ch)) = chars.next() {
            if ch != '{' {
                continue;
            }
            // Slice placeholders and separators directly from the format string.
            while chars.next_if(|(_, ch)| *ch != '}').is_some() {}
            let closing = chars.next().map(|(index, _)| index);
            let variable_end = closing.unwrap_or(format.len());
            let end = closing.map_or(format.len(), |index| index + 1);
            let variable = &format[start + 1..variable_end];
            let value = if variable == "time-ago" {
                Some(time_ago.as_deref())
            } else {
                self.get_variable(variable)
            };
            match value {
                Some(None) => {
                    // A missing first value suppresses its following separator;
                    // other missing values suppress their preceding separator.
                    if output.is_empty() {
                        exclude_next_literal = true;
                    }
                }
                value => {
                    if !exclude_next_literal {
                        output.push_str(&format[literal_start..start]);
                    }
                    exclude_next_literal = false;
                    output.push_str(value.flatten().unwrap_or(&format[start..end]));
                }
            }
            literal_start = end;
        }
        if !exclude_next_literal {
            output.push_str(&format[literal_start..]);
        }
        output
    }
}

#[cfg(all(test, feature = "git"))]
mod test {
    use super::*;
    use crate::git::test::create_commit_with_message;
    use crate::git::test::empty_git_repo;
    use std::fs::File;

    /// describes how a line was modified
    #[derive(PartialEq, PartialOrd, Ord, Eq)]
    enum LineDiff {
        /// this line is added
        Insert,
        /// this line is deleted
        Delete,
        /// no changes for this line
        None,
    }

    /// checks if the first argument is `no_commit` or not
    macro_rules! no_commit_flag {
        (no_commit, $commit_msg:literal) => {
            false
        };
        (, $commit_msg:literal) => {
            true
        };
        ($any:tt, $commit_msg:literal) => {
            compile_error!(concat!(
                "expected `no_commit` or nothing for commit ",
                $commit_msg
            ))
        };
    }

    /// checks if the first argument is `insert` or `delete`
    macro_rules! line_diff_flag {
        (insert, $commit_msg:literal, $line:expr) => {
            LineDiff::Insert
        };
        (delete, $commit_msg:literal, $line:expr) => {
            LineDiff::Delete
        };
        (, $commit_msg:literal, $line:expr) => {
            LineDiff::None
        };
        ($any:tt, $commit_msg:literal, $line:expr) => {
            compile_error!(concat!(
                "expected `insert`, `delete` or nothing for commit ",
                $commit_msg,
                " line ",
                $line
            ))
        };
    }

    /// This macro exists because we can't pass a `match` statement into `concat!`
    /// we would like to exclude any lines that are `delete`
    macro_rules! line_diff_flag_str {
        (insert, $commit_msg:literal, $line:expr) => {
            concat!($line, newline_literal!())
        };
        (delete, $commit_msg:literal, $line:expr) => {
            ""
        };
        (, $commit_msg:literal, $line:expr) => {
            concat!($line, newline_literal!())
        };
        ($any:tt, $commit_msg:literal, $line:expr) => {
            compile_error!(concat!(
                "expected `insert`, `delete` or nothing for commit ",
                $commit_msg,
                " line ",
                $line
            ))
        };
    }

    #[cfg(windows)]
    macro_rules! newline_literal {
        () => {
            "\r\n"
        };
    }
    #[cfg(not(windows))]
    macro_rules! newline_literal {
        () => {
            "\n"
        };
    }

    /// Helper macro to create a history of the same file being modified.
    macro_rules! assert_line_blame_progress {
        (
            $(
                // a unique identifier for the commit, other commits must not use this
                // If `no_commit` option is used, use the identifier of the previous commit
                $commit_msg:literal
                // must be `no_commit` if exists.
                // If exists, this block won't be committed
                $($no_commit:ident)? =>
                $(
                    // contents of a line in the file
                    $line:literal
                    // what commit identifier we are expecting for this line
                    $($expected:literal)?
                    // must be `insert` or `delete` if exists
                    // if exists, must be used with `no_commit`
                    // - `insert`: this line is added
                    // - `delete`: this line is deleted
                    $($line_diff:ident)?
                ),+
            );+
            $(;)?
        ) => {{
            use std::fs::OpenOptions;
            use std::io::Write;

            let repo = empty_git_repo();
            let file = repo.path().join("file.txt");
            File::create(&file).expect("could not create file");

            $(
                let file_content = concat!(
                    $(
                        line_diff_flag_str!($($line_diff)?, $commit_msg, $line),
                    )*
                );
                eprintln!("at commit {}:\n\n{file_content}", stringify!($commit_msg));

                let mut f = OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&file)
                    .unwrap();

                f.write_all(file_content.as_bytes()).unwrap();

                let should_commit = no_commit_flag!($($no_commit)?, $commit_msg);
                if should_commit {
                    create_commit_with_message(repo.path(), true, stringify!($commit_msg));
                }

                let mut line_number = 0;
                let mut added_lines = 0;
                let mut removed_lines = 0;

                $(
                    let line_diff_flag = line_diff_flag!($($line_diff)?, $commit_msg, $line);
                    #[allow(unused_assignments)]
                    match line_diff_flag {
                        LineDiff::Insert => added_lines += 1,
                        LineDiff::Delete => removed_lines += 1,
                        LineDiff::None => ()
                    }
                    // completely skip lines that are marked as `delete`
                    if line_diff_flag != LineDiff::Delete {
                        // if there is no $expected, then we don't care what blame_line returns
                        // because we won't show it to the user.
                        $(
                            let file_blame = FileBlame::try_new(file.clone(), true).unwrap();
                            let blame_result = file_blame
                                .blame_for_line(line_number - added_lines + removed_lines)
                                .and_then(|blame| blame.commit_title.as_deref());

                            assert_eq!(
                                blame_result,
                                Some(stringify!($expected)),
                                "Blame mismatch\nat commit: {}\nat line: {}\nline contents: {}\nexpected commit: {}\nbut got commit: {}",
                                $commit_msg,
                                line_number,
                                file_content
                                    .lines()
                                    .nth(line_number.try_into().unwrap())
                                    .unwrap(),
                                stringify!($expected),
                                blame_result
                                    .as_ref()
                                    .map(|blame| blame.trim_end())
                                    .unwrap_or("<no commit>")
                            );
                        )?
                        #[allow(unused_assignments)]
                        {
                            line_number += 1;
                        }
                    }
                )*
            )*
        }};
    }

    // For some reasons the CI is failing on windows with the message "Commits not found".
    // The created temporary repository has no commits... But this is not an issue on unix.
    // There is nothing platform-specific in this implementation. This is a problem only
    // for tests on Windows.
    // As such it should be fine to disable this test in Windows.
    // As long as these tests pass on other platforms, on Windows it will work too.
    #[cfg(not(windows))]
    #[test]
    pub fn blamed_lines() {
        assert_line_blame_progress! {
            // initialize
            1 =>
                "fn main() {" 1,
                "" 1,
                "}" 1;
            // modifying a line works
            2 =>
                "fn main() {" 1,
                "  one" 2,
                "}" 1;
            // inserting a line works
            3 =>
                "fn main() {" 1,
                "  one" 2,
                "  two" 3,
                "}" 1;
            // deleting a line works
            4 =>
                "fn main() {" 1,
                "  two" 3,
                "}" 1;
            // when a line is inserted in-between the blame order is preserved
            4 no_commit =>
                "fn main() {" 1,
                "  hello world" insert,
                "  two" 3,
                "}" 1;
            // Having a bunch of random lines interspersed should not change which lines
            // have blame for which commits
            4 no_commit =>
                "  six" insert,
                "  three" insert,
                "fn main() {" 1,
                "  five" insert,
                "  four" insert,
                "  two" 3,
                "  five" insert,
                "  four" insert,
                "}" 1,
                "  five" insert,
                "  four" insert;
            // committing all of those insertions should recognize that they are
            // from the current commit, while still keeping the information about
            // previous commits
            5 =>
                "  six" 5,
                "  three" 5,
                "fn main() {" 1,
                "  five" 5,
                "  four" 5,
                "  two" 3,
                "  five" 5,
                "  four" 5,
                "}" 1,
                "  five" 5,
                "  four" 5;
            // several lines deleted
            5 no_commit =>
                "  six" 5,
                "  three" 5,
                "fn main() {" delete,
                "  five" delete,
                "  four" delete,
                "  two" delete,
                "  five" delete,
                "  four" 5,
                "}" 1,
                "  five" 5,
                "  four" 5;
            // committing the deleted changes
            6 =>
                "  six" 5,
                "  three" 5,
                "  four" 5,
                "}" 1,
                "  five" 5,
                "  four" 5;
            // mixing inserts with deletes
            6 no_commit =>
                "  six" delete,
                "  2" insert,
                "  three" delete,
                "  four" 5,
                "  1" insert,
                "}" 1,
                "]" insert,
                "  five" delete,
                "  four" 5;
            // committing inserts and deletes
            7 =>
                "  2" 7,
                "  four" 5,
                "  1" 7,
                "}" 1,
                "]" 7,
                "  four" 5;
        };
    }

    #[test]
    fn linked_worktree_and_nested_paths_can_be_blamed_with_reduced_trust() {
        let repo = empty_git_repo();
        let nested = repo.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let file = nested.join("file.txt");
        std::fs::write(&file, "first\n").unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        crate::git::test::exec_git_cmd(
            &["worktree", "add", "-b", "linked", "worktree"],
            repo.path(),
        );
        let file = repo.path().join("worktree/nested/file.txt");
        // Windows canonical paths use a verbatim prefix unlike Git's metadata.
        for file in [file.clone(), file.canonicalize().unwrap()] {
            let blame = FileBlame::try_new(file, false).unwrap();
            assert_eq!(
                blame.blame_for_line(0).unwrap().parse_format("{title}"),
                "initial"
            );
        }
    }

    #[test]
    fn refresh_reuses_only_the_same_file_revision_and_trust() {
        let repo = empty_git_repo();
        let file = repo.path().join("file.txt");
        let other = repo.path().join("other.txt");
        std::fs::write(&file, "first\n").unwrap();
        std::fs::write(&other, "other\n").unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        let original = FileBlame::try_new(file.clone(), true).unwrap();
        let refreshed = FileBlame::try_refresh(file.clone(), true, Some(original.clone())).unwrap();
        assert!(Arc::ptr_eq(&original, &refreshed));
        let untrusted =
            FileBlame::try_refresh(file.clone(), false, Some(original.clone())).unwrap();
        assert!(!Arc::ptr_eq(&original, &untrusted));
        let different_file = FileBlame::try_refresh(other, true, Some(original.clone())).unwrap();
        assert!(!Arc::ptr_eq(&original, &different_file));
        // Even an unchanged blob must get fresh blame after a HEAD change.
        crate::git::test::exec_git_cmd(&["commit", "--allow-empty", "-m", "new HEAD"], repo.path());
        let new_head = FileBlame::try_refresh(file, true, Some(original.clone())).unwrap();
        assert!(!Arc::ptr_eq(&original, &new_head));
        assert_ne!(original.source.revision, new_head.source.revision);
    }

    #[test]
    fn ranges_share_commits_and_borrow_metadata() {
        let repo = empty_git_repo();
        let file = repo.path().join("file.txt");
        std::fs::write(&file, "first\nsecond\nthird\n").unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        std::fs::write(&file, "first\nchanged\nthird\n").unwrap();
        create_commit_with_message(repo.path(), true, "middle");
        let blame = FileBlame::try_new(file, true).unwrap();
        assert_eq!(blame.ranges.len(), 3);
        assert_eq!(blame.commits.len(), 2);
        assert!(std::ptr::eq(
            blame.blame_for_line(0).unwrap(),
            blame.blame_for_line(2).unwrap()
        ));
        assert_eq!(
            blame.blame_for_line(1).unwrap().parse_format("{title}"),
            "middle"
        );
        assert!(blame.blame_for_line(3).is_none());
    }

    #[test]
    fn single_commit_keeps_one_range_for_a_large_file() {
        let repo = empty_git_repo();
        let file = repo.path().join("file.txt");
        std::fs::write(&file, "line\n".repeat(10_000)).unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        let blame = FileBlame::try_new(file, true).unwrap();
        assert_eq!(blame.ranges.len(), 1);
        assert_eq!(blame.ranges[0].lines, 0..10_000);
        assert_eq!(blame.commits.len(), 1);
        assert!(blame.blame_for_line(9_999).is_some());
        assert!(blame.blame_for_line(10_000).is_none());
    }
}

#[cfg(test)]
mod format_tests {
    use super::LineBlame;

    #[test]
    fn format_borrows_metadata_with_a_large_unused_body() {
        let blame = LineBlame {
            commit_body: Some("x".repeat(1024 * 1024)),
            ..bob()
        };
        assert_eq!(blame.parse_format("{commit}"), "f14ab1cf");
        assert_eq!(blame.parse_format("{body}").len(), 1024 * 1024);
        assert_eq!(blame.parse_format("{time-ago} • {commit}"), "f14ab1cf");
        let dated = LineBlame {
            time_stamp: Some((0, 0)),
            ..blame
        };
        let relative = stdx::time::format_relative_time(0, 0);
        assert_eq!(
            dated.parse_format("{time-ago} / {time-ago}"),
            format!("{relative} / {relative}")
        );
    }

    #[test]
    fn format_preserves_literals_unknown_variables_and_unicode() {
        assert_eq!(bob().parse_format("literal text"), "literal text");
        assert_eq!(
            bob().parse_format("by {author} today"),
            "by Bob TheBuilder today"
        );
        assert_eq!(
            bob().parse_format("{unknown} → {title}"),
            "{unknown} → feat!: extend house"
        );
        assert_eq!(bob().parse_format("{unknown"), "{unknown");
        assert_eq!(bob().parse_format("{未知} 🌱"), "{未知} 🌱");
    }

    fn bob() -> LineBlame {
        LineBlame {
            commit_hash: Some("f14ab1cf".to_owned()),
            author_name: Some("Bob TheBuilder".to_owned()),
            author_email: Some("bob@bob.com".to_owned()),
            commit_date: Some("2028-01-10".to_owned()),
            commit_title: Some("feat!: extend house".to_owned()),
            commit_body: Some("BREAKING CHANGE: Removed door".to_owned()),
            time_stamp: None,
        }
    }

    #[test]
    fn inline_blame_format_parser() {
        let format = "{author}, {date} • {title} • {commit}";

        assert_eq!(
            bob().parse_format(format),
            "Bob TheBuilder, 2028-01-10 • feat!: extend house • f14ab1cf".to_owned()
        );
        assert_eq!(
            LineBlame {
                author_name: None,
                ..bob()
            }
            .parse_format(format),
            "2028-01-10 • feat!: extend house • f14ab1cf".to_owned()
        );
        assert_eq!(
            LineBlame {
                commit_date: None,
                ..bob()
            }
            .parse_format(format),
            "Bob TheBuilder • feat!: extend house • f14ab1cf".to_owned()
        );
        assert_eq!(
            LineBlame {
                commit_title: None,
                author_email: None,
                ..bob()
            }
            .parse_format(format),
            "Bob TheBuilder, 2028-01-10 • f14ab1cf".to_owned()
        );
        assert_eq!(
            LineBlame {
                commit_hash: None,
                ..bob()
            }
            .parse_format(format),
            "Bob TheBuilder, 2028-01-10 • feat!: extend house".to_owned()
        );
        assert_eq!(
            LineBlame {
                commit_date: None,
                author_name: None,
                ..bob()
            }
            .parse_format(format),
            "feat!: extend house • f14ab1cf".to_owned()
        );
        assert_eq!(
            LineBlame {
                author_name: None,
                commit_title: None,
                ..bob()
            }
            .parse_format(format),
            "2028-01-10 • f14ab1cf".to_owned()
        );
    }
}

#[cfg(all(test, not(feature = "git")))]
mod no_git_tests {
    #[test]
    fn blame_reports_when_git_support_is_disabled() {
        assert!(super::FileBlame::try_new("missing".into(), false)
            .unwrap_err()
            .to_string()
            .contains("not compiled in"));
    }
}

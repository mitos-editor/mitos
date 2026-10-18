use std::{cell::LazyCell, ops::Range, path::PathBuf, sync::Arc};

#[cfg(feature = "git")]
use std::collections::HashMap;

use anyhow::Result;

mod time;

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
                    commit_id: entry.commit_id.to_string(),
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
                    timestamp: time.map(|time| time.seconds),
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

#[derive(Clone, Default, Debug)]
pub struct LineBlame {
    /// Full object ID for copying and resolving the commit without ambiguity.
    pub commit_id: String,
    commit_hash: Option<String>,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub commit_date: Option<String>,
    pub commit_title: Option<String>,
    pub commit_body: Option<String>,
    /// Used to compute `time-ago`
    timestamp: Option<i64>,
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
            self.timestamp
                .map(|seconds| time::format_relative_time(seconds, time::now()))
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

#[cfg(test)]
mod test;

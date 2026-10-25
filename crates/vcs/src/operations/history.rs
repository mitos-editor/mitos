//! Bounded, read-only history queries. Cursors and file endpoints pin complete object IDs.
use super::*;

pub const HISTORY_PAGE_SIZE: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RevisionId(String);
impl RevisionId {
    pub fn parse(id: &str) -> Result<Self> {
        ensure!(
            matches!(id.len(), 40 | 64) && id.bytes().all(|c| c.is_ascii_hexdigit()),
            "Expected a complete Git object ID"
        );
        Ok(Self(id.to_ascii_lowercase()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn short(&self) -> &str {
        &self.0[..12]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionFile {
    pub revision: RevisionId,
    path: PathBuf,
}
impl RevisionFile {
    pub fn new(revision: RevisionId, path: PathBuf) -> Result<Self> {
        validate_path(&path)?;
        Ok(Self { revision, path })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryFile {
    pub before: Option<PathBuf>,
    pub after: Option<PathBuf>,
    pub status: String,
}
impl HistoryFile {
    pub fn path(&self) -> &Path {
        self.after.as_deref().or(self.before.as_deref()).unwrap()
    }
    pub fn endpoints(
        &self,
        commit: &CommitEntry,
        parent: usize,
    ) -> Result<(Option<RevisionFile>, Option<RevisionFile>)> {
        ensure!(
            parent < commit.parents.len().max(1),
            "Selected commit parent is unavailable"
        );
        let before = self
            .before
            .as_ref()
            .zip(commit.parents.get(parent))
            .map(|(path, id)| RevisionFile::new(id.clone(), path.clone()))
            .transpose()?;
        let after = self
            .after
            .as_ref()
            .map(|path| RevisionFile::new(commit.id.clone(), path.clone()))
            .transpose()?;
        Ok((before, after))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitEntry {
    pub id: RevisionId,
    pub parents: Vec<RevisionId>,
    pub author: String,
    pub date: String,
    pub subject: String,
    /// Paths at this commit and its first parent, including followed renames.
    pub file: Option<HistoryFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryCursor {
    head: RevisionId,
    path: Option<PathBuf>,
    skip: usize,
}

pub struct HistoryPage {
    pub commits: Vec<CommitEntry>,
    pub next: Option<HistoryCursor>,
}

impl GitRepository {
    /// Resolve HEAD once. Subsequent pages never consult the branch name again.
    pub fn history_cursor(&self, path: Option<&Path>) -> Result<Option<HistoryCursor>> {
        let repo = self.open(false)?;
        if repo.head()?.is_unborn() {
            return Ok(None);
        }
        let path = path.map(|path| self.relative(path)).transpose()?;
        Ok(Some(HistoryCursor {
            head: RevisionId::parse(&repo.head_commit()?.id.to_string())?,
            path,
            skip: 0,
        }))
    }

    /// Repository history walks the pinned ancestry; file history follows first parents for stable rename continuation.
    /// Dropping this future kills its Git process, including when a browser is hidden.
    pub async fn history_page(&self, cursor: HistoryCursor) -> Result<HistoryPage> {
        let mut command = self.command();
        command
            .args([
                "log",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--no-show-signature",
                "--format=%x00%H%x00%P%x00%an%x00%aI%x00%s%x00",
                "-z",
            ])
            .arg(format!("--max-count={}", HISTORY_PAGE_SIZE + 1));
        if cursor.path.is_some() {
            command.args([
                "--first-parent",
                "--diff-merges=first-parent",
                "--follow",
                "--name-status",
                "--find-renames=50%",
                "-l1000",
            ]);
        } else {
            command
                .args(["--topo-order", "--no-patch"])
                .arg(format!("--skip={}", cursor.skip));
        }
        command.arg(cursor.head.as_str()).arg("--");
        if let Some(path) = &cursor.path {
            command.arg(literal_pathspec(path));
        }
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(15), run(command))
            .await
            .context("Git history query timed out")??;
        let mut records = parse_log(&bytes)?;
        let has_more = records.len() > HISTORY_PAGE_SIZE;
        records.truncate(HISTORY_PAGE_SIZE);
        let mut path = cursor.path;
        let mut commits = Vec::with_capacity(records.len());
        for (mut commit, changes) in records {
            if let Some(current) = &path {
                let file = changes
                    .into_iter()
                    .find(|file| {
                        file.after.as_ref() == Some(current)
                            || file.before.as_ref() == Some(current)
                    })
                    .context("Git did not report the historical file path")?;
                // The next older commit sees the path at this commit's first parent.
                if let Some(before) = &file.before {
                    path = Some(before.clone());
                }
                commit.file = Some(file);
            }
            commits.push(commit);
        }
        let next = if !has_more {
            None
        } else if path.is_none() {
            Some(HistoryCursor {
                head: cursor.head,
                path: None,
                skip: cursor.skip + commits.len(),
            })
        } else {
            commits
                .last()
                .and_then(|commit| commit.parents.first())
                .map(|head| HistoryCursor {
                    head: head.clone(),
                    path,
                    skip: 0,
                })
        };
        Ok(HistoryPage { commits, next })
    }

    /// Compare a chosen parent with the commit, explicitly handling roots and merges.
    pub async fn commit_files(
        &self,
        commit: &CommitEntry,
        parent: usize,
    ) -> Result<Vec<HistoryFile>> {
        ensure!(
            parent < commit.parents.len().max(1),
            "Selected commit parent is unavailable"
        );
        let mut command = self.command();
        command.args([
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-status",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames=50%",
            "-l1000",
            "-r",
            "-z",
        ]);
        if let Some(parent) = commit.parents.get(parent) {
            command.arg(parent.as_str());
        }
        command.arg(commit.id.as_str()).arg("--");
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(15), run(command))
            .await
            .context("Git changed-file query timed out")??;
        parse_changes(&mut bytes.split(|byte| *byte == 0).peekable())
    }

    /// Read an exact historical blob without worktree conversions, filters, or external tools.
    pub fn read_revision_file(&self, file: &RevisionFile, trusted: bool) -> Result<Vec<u8>> {
        validate_path(file.path())?;
        let repo = self.open(trusted)?;
        let id = gix::ObjectId::from_hex(file.revision.as_str().as_bytes())?;
        let tree = repo
            .find_commit(id)
            .context("Historical commit is unavailable; it may have been pruned")?
            .tree()?;
        let entry = tree
            .lookup_entry_by_path(file.path())?
            .context("This file does not exist at the selected revision")?;
        crate::review::read_blob(&repo, entry.object_id(), entry.mode().kind(), file.path())
    }
}

fn validate_path(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
        "Expected a repository-relative historical file path"
    );
    Ok(())
}

fn parse_changes<'a>(
    fields: &mut std::iter::Peekable<impl Iterator<Item = &'a [u8]>>,
) -> Result<Vec<HistoryFile>> {
    let mut files = Vec::new();
    while let Some(status) = fields.peek().filter(|status| !status.is_empty()) {
        let status = status.strip_prefix(b"\n").unwrap_or(status);
        let kind = *status.first().context("Missing historical change kind")?;
        ensure!(
            matches!(kind, b'A' | b'D' | b'M' | b'T' | b'R' | b'C'),
            "Unsupported historical change kind"
        );
        let status = literal(status);
        fields.next();
        let path = fields.next().context("Missing historical file path")?;
        let path = gix::path::from_bstr(gix::bstr::BStr::new(path)).into_owned();
        validate_path(&path)?;
        let (before, after) = if matches!(kind, b'R' | b'C') {
            let new = fields
                .next()
                .context("Missing historical rename destination")?;
            let new = gix::path::from_bstr(gix::bstr::BStr::new(new)).into_owned();
            validate_path(&new)?;
            (Some(path), Some(new))
        } else {
            (
                (kind != b'A').then(|| path.clone()),
                (kind != b'D').then_some(path),
            )
        };
        files.push(HistoryFile {
            before,
            after,
            status,
        });
        ensure!(
            files.len() <= 20_000,
            "Commit contains too many files to display (limit 20000)"
        );
    }
    Ok(files)
}

fn parse_log(bytes: &[u8]) -> Result<Vec<(CommitEntry, Vec<HistoryFile>)>> {
    let mut fields = bytes.split(|byte| *byte == 0).peekable();
    let mut commits = Vec::new();
    while let Some(id) = fields.next() {
        if id.is_empty() {
            continue;
        }
        let id = RevisionId::parse(std::str::from_utf8(id)?)?;
        let parents = fields
            .next()
            .context("Missing commit parents")?
            .split(|byte| *byte == b' ')
            .filter(|id| !id.is_empty())
            .map(|id| RevisionId::parse(std::str::from_utf8(id)?))
            .collect::<Result<Vec<_>>>()?;
        let author = literal(fields.next().context("Missing commit author")?);
        let date = literal(fields.next().context("Missing commit date")?);
        let subject = literal(fields.next().context("Missing commit subject")?);
        // `-z` terminates the pretty record before its optional name-status records.
        if fields.peek() == Some(&b"".as_slice()) {
            fields.next();
        }
        let changes = if fields.peek().is_some_and(|field| field.starts_with(b"\n")) {
            parse_changes(&mut fields)?
        } else {
            Vec::new()
        };
        commits.push((
            CommitEntry {
                id,
                parents,
                author,
                date,
                subject,
                file: None,
            },
            changes,
        ));
        ensure!(
            commits.len() <= HISTORY_PAGE_SIZE + 1,
            "Git returned too many history rows"
        );
    }
    Ok(commits)
}

#[cfg(test)]
mod tests;

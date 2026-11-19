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
    /// Committer timestamp in UTC, matching the traversal's commit-time priority.
    pub date: String,
    pub subject: String,
    /// Paths at this commit and its first parent, including followed renames.
    pub file: Option<HistoryFile>,
}

#[derive(Clone, Debug)]
pub struct HistoryCursor {
    source: std::sync::Arc<walk::Source>,
    offset: usize,
}

impl HistoryCursor {
    /// Release the native walk, object cache and replay rows. The next read
    /// rebuilds from the pinned HEAD and original path to this cursor's offset.
    pub fn suspend(&self) {
        self.source.suspend();
    }
}

pub struct HistoryPage {
    pub commits: Vec<std::sync::Arc<CommitEntry>>,
    pub next: Option<HistoryCursor>,
}

impl GitRepository {
    /// Pin HEAD without loading commit objects on the UI thread. Clones and
    /// continuations share one native walk and can retry already published pages.
    pub fn history_cursor(&self, path: Option<&Path>) -> Result<Option<HistoryCursor>> {
        let repo = self.open_read(false)?;
        let head = repo.head()?;
        let Some(id) = head.id() else {
            return Ok(None);
        };
        let path = path.map(|path| self.relative(path)).transpose()?;
        Ok(Some(HistoryCursor {
            source: std::sync::Arc::new(walk::Source::new(self.clone(), id.detach(), path)),
            offset: 0,
        }))
    }

    /// Request a page from the persistent native walk. Dropping the future stops
    /// the current read cooperatively; the cursor retains progress for restoration.
    pub async fn history_page(&self, cursor: HistoryCursor) -> Result<HistoryPage> {
        ensure!(
            cursor.source.belongs_to(self),
            "History cursor belongs to another repository"
        );
        let (commits, more) = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            cursor.source.page(cursor.offset),
        )
        .await
        .context("Git history query timed out")??;
        let offset = cursor.offset + commits.len();
        Ok(HistoryPage {
            commits,
            next: more.then(|| HistoryCursor {
                source: cursor.source,
                offset,
            }),
        })
    }

    /// Load the selected commit's full message without retaining bodies in history pages.
    pub async fn commit_message(&self, revision: &RevisionId) -> Result<String> {
        let repository = self.clone();
        let revision = revision.clone();
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            tokio::task::spawn_blocking(move || {
                let repo = repository.open_read(false)?;
                let commit =
                    repo.find_commit(gix::ObjectId::from_hex(revision.as_str().as_bytes())?)?;
                let message = commit.message_raw()?;
                ensure!(
                    message.len() <= 1024 * 1024,
                    "Commit message exceeds the 1 MiB display limit"
                );
                Ok(String::from_utf8_lossy(message.as_ref()).into_owned())
            }),
        )
        .await
        .context("Git commit-message query timed out")?
        .context("History commit-message worker stopped")?
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
        let repository = self.clone();
        let commit = commit.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::task::spawn_blocking(move || {
            if tx.is_closed() {
                return;
            }
            let result = (|| {
                let repo = repository.open_read(false)?;
                let tree = repo
                    .find_commit(gix::ObjectId::from_hex(commit.id.as_str().as_bytes())?)?
                    .tree()?;
                let before = match commit.parents.get(parent) {
                    Some(id) => repo
                        .find_commit(gix::ObjectId::from_hex(id.as_str().as_bytes())?)?
                        .tree()?,
                    None => repo.empty_tree(),
                };
                changes::between(&before, &tree, || tx.is_closed())
            })();
            let _ = tx.send(result);
        });
        tokio::time::timeout(std::time::Duration::from_secs(15), rx)
            .await
            .context("Git changed-file query timed out")?
            .context("History changed-file worker stopped")?
    }

    /// Read an exact historical blob without worktree conversions, filters, or external tools.
    pub fn read_revision_file(&self, file: &RevisionFile, trusted: bool) -> Result<Vec<u8>> {
        validate_path(file.path())?;
        let repo = self.open_read(trusted)?;
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

mod changes;
mod walk;

#[cfg(test)]
mod tests;

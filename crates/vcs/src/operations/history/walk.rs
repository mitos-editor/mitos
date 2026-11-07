//! One demand-driven worker owns the repository and its borrowed gix walker.
//! Cursor clones share the visited prefix, so cancellation and retry cannot skip rows.
use super::*;
use std::sync::Mutex;
use tokio::sync::{mpsc, oneshot};

type Batch = (Vec<CommitEntry>, bool);
const MAX_PAGE_BYTES: usize = 1024 * 1024;

fn metadata_size(row: &CommitEntry) -> usize {
    row.id.as_str().len()
        + row
            .parents
            .iter()
            .map(|id| id.as_str().len())
            .sum::<usize>()
        + row.author.len()
        + row.date.len()
        + row.subject.len()
        + row.file.as_ref().map_or(0, |file| {
            file.before
                .iter()
                .chain(&file.after)
                .map(|path| path.as_os_str().as_encoded_bytes().len())
                .sum::<usize>()
        })
}
struct Request {
    offset: usize,
    reply: oneshot::Sender<Result<Batch>>,
}

pub(super) struct Source {
    repository: GitRepository,
    head: gix::ObjectId,
    path: Option<PathBuf>,
    sender: Mutex<Option<mpsc::Sender<Request>>>,
}
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistorySource")
            .field("head", &self.head)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}
impl Source {
    pub(super) fn new(
        repository: GitRepository,
        head: gix::ObjectId,
        path: Option<PathBuf>,
    ) -> Self {
        Self {
            repository,
            head,
            path,
            sender: Mutex::new(None),
        }
    }
    pub(super) fn belongs_to(&self, repository: &GitRepository) -> bool {
        self.repository.root == repository.root
            && self.repository.git_dir == repository.git_dir
            && self.repository.common_dir == repository.common_dir
    }
    pub(super) async fn page(&self, offset: usize) -> Result<Batch> {
        let (reply, receiver) = oneshot::channel();
        let sender = {
            let mut sender = self
                .sender
                .lock()
                .map_err(|_| anyhow::anyhow!("History worker lock poisoned"))?;
            if sender.as_ref().is_none_or(|sender| sender.is_closed()) {
                let (tx, rx) = mpsc::channel(1);
                let repository = self.repository.clone();
                let head = self.head;
                let path = self.path.clone();
                std::thread::Builder::new()
                    .name("git-history".into())
                    .spawn(move || run_worker(repository, head, path, rx))?;
                *sender = Some(tx);
            }
            sender.as_ref().unwrap().clone()
        };
        sender
            .send(Request { offset, reply })
            .await
            .map_err(|_| anyhow::anyhow!("History worker stopped; reopen to retry"))?;
        receiver.await.context("History worker stopped")?
    }
}

fn run_worker(
    repository: GitRepository,
    head: gix::ObjectId,
    path: Option<PathBuf>,
    mut requests: mpsc::Receiver<Request>,
) {
    let first = loop {
        match requests.blocking_recv() {
            Some(request) if !request.reply.is_closed() => break request,
            Some(_) => continue,
            None => return,
        }
    };
    let mut repo = match repository.open_history(false) {
        Ok(repo) => repo,
        Err(error) => {
            let _ = first.reply.send(Err(error));
            return;
        }
    };
    repo.object_cache_size(Some(32 * 1024 * 1024));
    let mut platform = repo
        .rev_walk([head])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ));
    if path.is_some() {
        platform = platform.first_parent_only();
    }
    let walk = match platform.all() {
        Ok(walk) => walk,
        Err(error) => {
            let _ = first.reply.send(Err(error.into()));
            return;
        }
    };
    let mut source = Walk {
        repo: &repo,
        walk,
        path,
        rows: Vec::new(),
        exhausted: false,
        pending: None,
    };
    let mut request = Some(first);
    while let Some(current) = request {
        let result = source.page(current.offset, || current.reply.is_closed());
        let failed = result.is_err();
        if let Some(result) = result.transpose() {
            let _ = current.reply.send(result);
        }
        if failed {
            break;
        }
        request = requests.blocking_recv();
    }
}

struct Walk<'a> {
    repo: &'a gix::Repository,
    walk: gix::revision::Walk<'a>,
    path: Option<PathBuf>,
    rows: Vec<CommitEntry>,
    exhausted: bool,
    pending: Option<gix::ObjectId>,
}
impl Walk<'_> {
    fn page(&mut self, offset: usize, canceled: impl Fn() -> bool) -> Result<Option<Batch>> {
        let target = offset + HISTORY_PAGE_SIZE + 1;
        let mut page_bytes: usize = self
            .rows
            .get(offset..self.rows.len().min(target))
            .unwrap_or(&[])
            .iter()
            .map(metadata_size)
            .sum();
        while !self.exhausted && self.rows.len() < target {
            if canceled() {
                return Ok(None);
            }
            let id = match self.pending {
                Some(id) => id,
                None => match self.walk.next().transpose()? {
                    Some(info) => {
                        self.pending = Some(info.id);
                        info.id
                    }
                    None => {
                        self.exhausted = true;
                        break;
                    }
                },
            };
            let commit = self.repo.find_commit(id)?;
            let parents = commit
                .parent_ids()
                .map(|id| RevisionId::parse(&id.to_string()))
                .collect::<Result<Vec<_>>>()?;
            let file = if let Some(path) = &self.path {
                let after = commit.tree()?;
                let before = match parents.first() {
                    Some(parent) => self
                        .repo
                        .find_commit(gix::ObjectId::from_hex(parent.as_str().as_bytes())?)?
                        .tree()?,
                    None => self.repo.empty_tree(),
                };
                let entry = |tree: &gix::Tree<'_>| -> Result<_> {
                    Ok(tree
                        .lookup_entry_by_path(path)?
                        .map(|e| (e.object_id(), e.mode())))
                };
                if entry(&before)? == entry(&after)? {
                    self.pending = None;
                    continue;
                }
                let files = changes::between(&before, &after, &canceled);
                if canceled() {
                    return Ok(None);
                }
                let Some(file) = files?.into_iter().find(|file| {
                    file.after.as_ref() == Some(path) || file.before.as_ref() == Some(path)
                }) else {
                    self.pending = None;
                    continue;
                };
                if let Some(before) = &file.before {
                    self.path = Some(before.clone());
                }
                Some(file)
            } else {
                None
            };
            let author = literal(commit.author()?.name.as_ref());
            let time = commit.committer()?.time.parse::<gix::date::Time>()?;
            let date = gix::date::Time::new(time.seconds, 0)
                .format(gix::date::time::format::ISO8601_STRICT)?;
            let subject = literal(commit.message()?.summary().as_ref());
            let row = CommitEntry {
                id: RevisionId::parse(&id.to_string())?,
                parents,
                author,
                date,
                subject,
                file,
            };
            page_bytes += metadata_size(&row);
            ensure!(
                page_bytes <= MAX_PAGE_BYTES,
                "History page exceeds the 1 MiB display limit"
            );
            self.pending = None;
            self.rows.push(row);
        }
        let end = self.rows.len().min(offset + HISTORY_PAGE_SIZE);
        ensure!(offset <= end, "Invalid history continuation");
        Ok(Some((
            self.rows[offset..end].to_vec(),
            self.rows.len() > end,
        )))
    }
}

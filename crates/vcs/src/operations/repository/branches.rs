//! A demand-driven worker keeps one native reference iterator across pages.
use super::*;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

pub const BRANCH_PAGE_SIZE: usize = 100;
type Batch = (Vec<Arc<Branch>>, bool);
struct Request {
    offset: usize,
    reply: oneshot::Sender<Result<Batch>>,
}

#[derive(Clone)]
pub struct BranchCursor {
    source: Arc<Source>,
    offset: usize,
}

pub struct BranchPage {
    pub branches: Vec<Arc<Branch>>,
    pub next: Option<BranchCursor>,
}

struct Source {
    repository: GitRepository,
    sender: Mutex<Option<mpsc::Sender<Request>>>,
}

impl GitRepository {
    /// Create a lazy listing; repository and reference reads run off the UI thread.
    pub fn branch_cursor(&self) -> BranchCursor {
        BranchCursor {
            source: Arc::new(Source {
                repository: self.clone(),
                sender: Mutex::new(None),
            }),
            offset: 0,
        }
    }
}

impl BranchCursor {
    /// Cursor clones can retry canceled pages without skipping any references.
    /// A failed worker is restarted and replays the live listing to this offset;
    /// callers should create a new cursor when changing the repository's references.
    pub async fn next_page(&self) -> Result<BranchPage> {
        let (reply, receiver) = oneshot::channel();
        let sender = {
            let mut sender = self
                .source
                .sender
                .lock()
                .map_err(|_| anyhow::anyhow!("Branch worker lock poisoned"))?;
            if sender.as_ref().is_none_or(|sender| sender.is_closed()) {
                let (tx, rx) = mpsc::channel(1);
                let repository = self.source.repository.clone();
                std::thread::Builder::new()
                    .name("git-branches".into())
                    .spawn(move || run(repository, rx))?;
                *sender = Some(tx);
            }
            sender.as_ref().unwrap().clone()
        };
        let read = async {
            sender
                .send(Request {
                    offset: self.offset,
                    reply,
                })
                .await
                .map_err(|_| anyhow::anyhow!("Branch worker stopped; retry the page"))?;
            receiver.await.context("Branch worker stopped")?
        };
        let (branches, more) = tokio::time::timeout(std::time::Duration::from_secs(15), read)
            .await
            .context("Git branch query timed out")??;
        let offset = self.offset + branches.len();
        Ok(BranchPage {
            branches,
            next: more.then(|| Self {
                source: self.source.clone(),
                offset,
            }),
        })
    }
}

fn run(repository: GitRepository, mut requests: mpsc::Receiver<Request>) {
    let Some(first) = requests.blocking_recv() else {
        return;
    };
    let mut first = Some(first);
    let result = (|| -> Result<()> {
        let repo = repository.open_read(false)?;
        let head = repo.head_name()?;
        let platform = repo.references()?;
        let mut references = platform.local_branches()?;
        let mut rows = Vec::new();
        let mut exhausted = false;
        let first = first.take().unwrap();
        let mut request = Some(first.reply);
        let mut offset = first.offset;
        loop {
            let reply = request.take().unwrap();
            let result = (|| -> Result<Option<Batch>> {
                let mut bytes: usize = rows
                    .get(offset..rows.len().min(offset + BRANCH_PAGE_SIZE + 1))
                    .unwrap_or(&[])
                    .iter()
                    .map(|row: &Arc<Branch>| row.name.len() + row.upstream.len())
                    .sum();
                while !exhausted && rows.len() < offset + BRANCH_PAGE_SIZE + 1 {
                    if reply.is_closed() {
                        return Ok(None);
                    }
                    let Some(reference) = references.next() else {
                        exhausted = true;
                        break;
                    };
                    let reference = reference?;
                    let upstream = upstream_name(&repo, reference.name())?;
                    let name = reference.name().shorten().to_str()?.to_owned();
                    // Rebuilding a failed worker may need to replay earlier pages.
                    // Their metadata does not count against the requested page's budget.
                    if rows.len() >= offset {
                        bytes += name.len() + upstream.len();
                    }
                    ensure!(
                        bytes <= 1024 * 1024,
                        "Branch page exceeds the 1 MiB display limit"
                    );
                    rows.push(Arc::new(Branch {
                        current: head
                            .as_ref()
                            .is_some_and(|head| head.as_ref() == reference.name()),
                        name,
                        upstream,
                    }));
                }
                let end = rows.len().min(offset + BRANCH_PAGE_SIZE);
                ensure!(offset <= end, "Branch listing changed; reopen to refresh");
                Ok(Some((
                    rows[offset..end].to_vec(),
                    !exhausted || end < rows.len(),
                )))
            })();
            let failed = result.is_err();
            // Make failure observable before waking the caller, so an immediate
            // retry always replaces this worker rather than using its dying channel.
            if failed {
                requests.close();
            }
            if let Some(result) = result.transpose() {
                let _ = reply.send(result);
            }
            if failed {
                break;
            }
            let Some(next) = requests.blocking_recv() else {
                break;
            };
            offset = next.offset;
            request = Some(next.reply);
        }
        Ok(())
    })();
    // Initialization failures must reach the waiting caller as well.
    if let Err(error) = result {
        requests.close();
        let _ = first.unwrap().reply.send(Err(error));
    }
}

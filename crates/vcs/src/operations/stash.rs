use super::*;

/// Identity is immutable. The reflog selector is for display only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StashEntry {
    pub(super) id: String,
    pub selector: String,
    pub message: String,
}

impl StashEntry {
    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StashLayer {
    Staged,
    Worktree,
    Untracked,
}
impl StashLayer {
    pub fn label(self) -> &'static str {
        match self {
            Self::Staged => "saved index",
            Self::Worktree => "saved worktree",
            Self::Untracked => "untracked",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StashFile {
    path: PathBuf,
    layer: StashLayer,
}

impl StashFile {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn untracked(&self) -> bool {
        self.layer == StashLayer::Untracked
    }
    pub fn layer(&self) -> StashLayer {
        self.layer
    }
}

pub(super) struct StashTrees<'a> {
    base: gix::Tree<'a>,
    worktree: gix::Tree<'a>,
    index: gix::Tree<'a>,
    untracked: Option<gix::Tree<'a>>,
}

pub(super) fn trees<'a>(repo: &'a gix::Repository, id: &str) -> Result<StashTrees<'a>> {
    let id = gix::ObjectId::from_hex(id.as_bytes())?;
    let commit = repo.find_commit(id)?;
    let parents: Vec<_> = commit.parent_ids().collect();
    ensure!(
        (2..=3).contains(&parents.len()),
        "Selected object is not a stash commit"
    );
    Ok(StashTrees {
        base: parents[0].object()?.try_into_commit()?.tree()?,
        worktree: commit.tree()?,
        index: parents[1].object()?.try_into_commit()?.tree()?,
        untracked: parents
            .get(2)
            .map(|parent| Ok::<_, anyhow::Error>(parent.object()?.try_into_commit()?.tree()?))
            .transpose()?,
    })
}

impl GitRepository {
    /// Keep the list bounded. Newer entries come first; no filters or external diff tools run.
    pub async fn stashes(&self) -> Result<Vec<StashEntry>> {
        let repository = self.clone();
        tokio::task::spawn_blocking(move || {
            let repo = repository.open_read(false)?;
            let Some(stash) = repo.try_find_reference("refs/stash")? else {
                return Ok(Vec::new());
            };
            let mut logs = stash.log_iter();
            // Read from the end without loading the entire stash reflog. Keep the
            // same 1 MiB bound as other repository listings, even for long messages.
            logs.buf.resize(1024 * 1024, 0);
            let Some(log) = logs.store.reflog_iter_rev(logs.name, &mut logs.buf)? else {
                return Ok(Vec::new());
            };
            let mut entries = Vec::new();
            let mut bytes = 0;
            for (index, line) in log.take(500).enumerate() {
                let line = line?;
                let id = line.new_oid.to_string();
                bytes += id.len() + line.message.len();
                ensure!(
                    bytes <= 1024 * 1024,
                    "Stash listing exceeds the 1 MiB display limit"
                );
                entries.push(StashEntry {
                    id,
                    selector: format!("stash@{{{index}}}"),
                    message: literal(&line.message),
                });
            }
            Ok(entries)
        })
        .await?
    }

    /// Inspect tracked changes and separately stored untracked files without rename scanning.
    pub async fn stash_files(&self, entry: &StashEntry) -> Result<Vec<StashFile>> {
        let id = entry.id.clone();
        let repository = self.clone();
        tokio::task::spawn_blocking(move || {
            let repo = repository.open_read(false)?;
            let trees = trees(&repo, &id)?;
            let mut files = Vec::new();
            for (after, layer) in [
                (&trees.index, StashLayer::Staged),
                (&trees.worktree, StashLayer::Worktree),
            ] {
                changed_paths(&trees.base, after, layer, &mut files)?;
            }
            if let Some(untracked) = &trees.untracked {
                changed_paths(
                    &repo.empty_tree(),
                    untracked,
                    StashLayer::Untracked,
                    &mut files,
                )?;
            }
            files.sort_by(|left, right| {
                left.path
                    .cmp(&right.path)
                    .then(left.layer.cmp(&right.layer))
            });
            Ok(files)
        })
        .await?
    }

    pub fn read_stash_file(
        &self,
        entry: &StashEntry,
        file: &StashFile,
        trusted: bool,
    ) -> Result<crate::review::Comparison> {
        let repo = self.open_read(trusted)?;
        let trees = trees(&repo, &entry.id)?;
        let read = |tree: &gix::Tree<'_>| -> Result<Vec<u8>> {
            match tree.lookup_entry_by_path(&file.path)? {
                Some(entry) => crate::review::read_blob(
                    &repo,
                    entry.object_id(),
                    entry.mode().kind(),
                    &file.path,
                ),
                None => Ok(Vec::new()),
            }
        };
        let (before, after) = match file.layer {
            StashLayer::Untracked => (
                Vec::new(),
                read(
                    trees
                        .untracked
                        .as_ref()
                        .context("Stash has no untracked tree")?,
                )?,
            ),
            StashLayer::Staged => (read(&trees.base)?, read(&trees.index)?),
            StashLayer::Worktree => (read(&trees.base)?, read(&trees.worktree)?),
        };
        Ok(crate::review::Comparison { before, after })
    }
}

fn changed_paths(
    before: &gix::Tree<'_>,
    after: &gix::Tree<'_>,
    layer: StashLayer,
    files: &mut Vec<StashFile>,
) -> Result<()> {
    use gix::object::tree::diff::Change;
    use std::ops::ControlFlow;
    let mut diff = before.changes()?;
    diff.options(|options| {
        options.track_path().track_rewrites(None);
    });
    let mut bytes = 0;
    diff.for_each_to_obtain_tree(after, |change| -> gix::Result<_> {
        let location = match change {
            Change::Addition {
                location,
                entry_mode,
                ..
            }
            | Change::Deletion {
                location,
                entry_mode,
                ..
            }
            | Change::Modification {
                location,
                entry_mode,
                ..
            } if !entry_mode.is_tree() => location,
            _ => return Ok(ControlFlow::Continue(())),
        };
        let path = gix::path::from_bstr(location)?.into_owned();
        gix::error::ensure!(
            path.components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
            "Invalid stash file path"
        );
        bytes += location.len();
        gix::error::ensure!(
            bytes <= 1024 * 1024,
            "Stash file listing exceeds the 1 MiB display limit"
        );
        gix::error::ensure!(
            files.len() < 20_000,
            "Stash contains too many files to display (limit 20000)"
        );
        files.push(StashFile { path, layer });
        Ok(ControlFlow::Continue(()))
    })?;
    Ok(())
}

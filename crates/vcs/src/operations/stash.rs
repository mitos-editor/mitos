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
        let mut command = self.command();
        command.args([
            "stash",
            "list",
            "--max-count=500",
            "--format=%H%x00%gd%x00%gs",
            "-z",
        ]);
        let bytes = run(command).await?;
        let mut entries = Vec::new();
        let mut fields = bytes.split(|byte| *byte == 0);
        while let Some(id) = fields.next().filter(|id| !id.is_empty()) {
            ensure!(
                matches!(id.len(), 40 | 64) && id.iter().all(u8::is_ascii_hexdigit),
                "Invalid stash object ID"
            );
            let selector = fields.next().context("Missing stash selector")?;
            let message = fields.next().context("Missing stash message")?;
            entries.push(StashEntry {
                id: String::from_utf8(id.to_vec())?,
                selector: literal(selector),
                message: literal(message),
            });
        }
        Ok(entries)
    }

    /// Inspect tracked changes and separately stored untracked files without rename scanning.
    pub async fn stash_files(&self, entry: &StashEntry) -> Result<Vec<StashFile>> {
        let id = entry.id.clone();
        let repository = self.clone();
        let (base, worktree, index, untracked) = tokio::task::spawn_blocking(move || {
            let repo = repository.open(false)?;
            let trees = trees(&repo, &id)?;
            Ok::<_, anyhow::Error>((
                trees.base.id.to_string(),
                trees.worktree.id.to_string(),
                trees.index.id.to_string(),
                trees.untracked.map(|tree| tree.id.to_string()),
            ))
        })
        .await??;
        let mut files = Vec::new();
        for (tree, layer) in [
            (index, StashLayer::Staged),
            (worktree, StashLayer::Worktree),
        ] {
            let mut command = self.command();
            command
                .args([
                    "diff-tree",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-commit-id",
                    "--name-only",
                    "--no-renames",
                    "-r",
                    "-z",
                ])
                .arg(&base)
                .arg(tree);
            files.extend(paths(&run(command).await?, layer)?);
        }
        if let Some(untracked) = untracked {
            let mut command = self.command();
            command
                .args(["ls-tree", "--name-only", "-r", "-z"])
                .arg(untracked);
            files.extend(paths(&run(command).await?, StashLayer::Untracked)?);
        }
        ensure!(
            files.len() <= 20_000,
            "Stash contains too many files to display (limit 20000)"
        );
        files.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.layer.cmp(&right.layer))
        });
        Ok(files)
    }

    pub fn read_stash_file(
        &self,
        entry: &StashEntry,
        file: &StashFile,
        trusted: bool,
    ) -> Result<crate::review::Comparison> {
        let repo = self.open(trusted)?;
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

fn paths(bytes: &[u8], layer: StashLayer) -> Result<Vec<StashFile>> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            let path = gix::path::from_bstr(gix::bstr::BStr::new(path));
            ensure!(
                path.components()
                    .all(|part| matches!(part, std::path::Component::Normal(_))),
                "Invalid stash file path"
            );
            Ok(StashFile {
                path: path.into_owned(),
                layer,
            })
        })
        .collect()
}

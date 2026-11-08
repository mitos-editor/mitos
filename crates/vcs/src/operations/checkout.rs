//! Two-tree branch checkout: prepare filtered files before touching the worktree,
//! preserve unrelated index/worktree edits, and lock refs and index during installation.
use super::*;
use gix::{
    bstr::{BString, ByteSlice},
    refs::transaction::{PreviousValue, RefEdit},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};

type Entries = BTreeMap<BString, (gix::ObjectId, gix::index::entry::Mode)>;

fn entries(index: &gix::index::File) -> Entries {
    index
        .entries()
        .iter()
        .map(|entry| (entry.path(index).to_owned(), (entry.id, entry.mode)))
        .collect()
}

impl PreparedOperation {
    pub(super) fn switch_branch(&self, name: &str) -> Result<()> {
        self.checkout(name, None).map(|_| ())
    }

    /// Return false before mutation when Git must handle an unsupported index or submodule.
    pub(super) fn fast_forward(
        &self,
        name: &str,
        expected: Option<gix::ObjectId>,
        target: gix::ObjectId,
    ) -> Result<bool> {
        self.checkout(name, Some((expected, target)))
    }

    fn checkout(
        &self,
        name: &str,
        fast_forward: Option<(Option<gix::ObjectId>, gix::ObjectId)>,
    ) -> Result<bool> {
        let mut repo = self.repository.open_read(true)?;
        repo.committer_or_set_generic_fallback()?;
        let refname: gix::refs::FullName = format!("refs/heads/{name}").try_into()?;
        let head: gix::refs::Target = repo.find_reference("HEAD")?.target().into();
        let (target_tree, edits) = if let Some((expected, target)) = fast_forward {
            ensure!(
                head == gix::refs::Target::Symbolic(refname.clone())
                    && repository::current_head(&repo)? == expected
                    && repo.state().is_none(),
                "Git branch or HEAD changed during fetch; retry pull"
            );
            // Preserve Git's hook execution and signature verification until the native
            // checkout can provide those semantics as well.
            let config = repo.config_snapshot();
            if config.string("core.hooksPath").is_some()
                || config.boolean("merge.verifySignatures").unwrap_or(false)
                || ["post-merge", "reference-transaction"]
                    .iter()
                    .any(|name| repo.common_dir().join("hooks").join(name).exists())
            {
                return Ok(false);
            }
            if let Some(current) = expected {
                let base = repo.merge_base(current, target)?.map(|id| id.detach());
                if base == Some(target) {
                    let index = repo.index_or_empty()?;
                    if !index::can_write(&repo, &index) {
                        return Ok(false);
                    }
                    self.ensure_current()?;
                    // Git records an undo point even when already up to date. Lock
                    // unchanged HEAD and its branch too, so a concurrent switch or
                    // ref update cannot publish an undo point for a different branch.
                    repo.edit_references([
                        RefEdit::update(
                            "HEAD".try_into()?,
                            head.clone(),
                            PreviousValue::MustExistAndMatch(head),
                            "",
                        ),
                        RefEdit::update(
                            refname,
                            current,
                            PreviousValue::MustExistAndMatch(current.into()),
                            "",
                        ),
                        RefEdit::update(
                            "ORIG_HEAD".try_into()?,
                            current,
                            PreviousValue::Any,
                            "pull: already up to date",
                        ),
                    ])?;
                    return Ok(true);
                }
                ensure!(
                    base == Some(current),
                    "Pull requires a fast-forward; branches have diverged"
                );
            }
            let message = "pull: fast-forward";
            // Dereferencing HEAD locks both HEAD and the branch, and writes both reflogs.
            let mut edits = vec![RefEdit::update(
                "HEAD".try_into()?,
                target,
                expected.map_or(PreviousValue::MustNotExist, |id| {
                    PreviousValue::MustExistAndMatch(id.into())
                }),
                message,
            )
            .with_deref(true)];
            if let Some(previous) = expected {
                edits.push(RefEdit::update(
                    "ORIG_HEAD".try_into()?,
                    previous,
                    PreviousValue::Any,
                    message,
                ));
            }
            (repo.find_commit(target)?.tree_id()?.detach(), edits)
        } else {
            if head == gix::refs::Target::Symbolic(refname.clone()) {
                return Ok(true);
            }
            branch::ensure_branch_unused(&repo, refname.as_ref())?;
            let mut branch = repo.find_reference(refname.as_ref())?;
            let target_ref: gix::refs::Target = branch.target().into();
            let tree = branch.peel_to_commit()?.tree_id()?.detach();
            let edits = vec![
                RefEdit::update(
                    "HEAD".try_into()?,
                    gix::refs::Target::Symbolic(refname.clone()),
                    PreviousValue::MustExistAndMatch(head),
                    format!("checkout: moving to {name}"),
                )
                .with_deref(false),
                RefEdit::update(
                    refname.clone(),
                    target_ref.clone(),
                    PreviousValue::MustExistAndMatch(target_ref),
                    "",
                )
                .with_deref(false),
            ];
            (tree, edits)
        };
        let mut target = repo.index_from_tree(&target_tree)?;
        let original = repo.index_or_empty()?;
        if fast_forward.is_some() && !index::can_write(&repo, &original) {
            return Ok(false);
        }
        ensure!(
            index::can_write(&repo, &original),
            "Resolve index conflicts or disable sparse checkout before switching branches"
        );
        let before = if repo.head()?.is_unborn() {
            Entries::new()
        } else {
            entries(&repo.index_from_tree(&repo.head_tree_id()?)?)
        };
        let staged = entries(&original);
        let after = entries(&target);
        let mut changed = BTreeSet::new();
        for path in before.keys().chain(staged.keys()).chain(after.keys()) {
            if before.get(path) == after.get(path) || staged.get(path) == after.get(path) {
                continue;
            }
            ensure!(
                staged.get(path) == before.get(path),
                "Checkout would overwrite staged changes at {path}"
            );
            for index in [&*original, &target] {
                if let Some(entry) = index.entry_by_path(path.as_bstr()) {
                    if fast_forward.is_some()
                        && (entry.mode.is_submodule()
                            || entry.flags.intersects(
                                gix::index::entry::Flags::ASSUME_VALID
                                    | gix::index::entry::Flags::SKIP_WORKTREE
                                    | gix::index::entry::Flags::INTENT_TO_ADD,
                            ))
                    {
                        return Ok(false);
                    }
                    ensure!(
                        !entry.mode.is_submodule(),
                        "Switching a changed submodule requires a terminal checkout"
                    );
                    ensure!(
                        !entry.flags.intersects(
                            gix::index::entry::Flags::ASSUME_VALID
                                | gix::index::entry::Flags::SKIP_WORKTREE
                                | gix::index::entry::Flags::INTENT_TO_ADD
                        ),
                        "Checkout would affect an index entry with hidden changes at {path}"
                    );
                }
            }
            for component in path.split(|byte| *byte == b'/') {
                gix::validate::path::component(component.as_bstr(), None, Default::default())?;
            }
            changed.insert(path.clone());
        }
        // Compare worktree to index separately: staged changes on unaffected paths are allowed.
        for item in repo
            .status(gix::progress::Discard)?
            .untracked_files(gix::status::UntrackedFiles::None)
            .index_worktree_rewrites(None)
            .tree_index_track_renames(gix::status::tree_index::TrackRenames::Disabled)
            .into_iter(None)?
        {
            let item = item?;
            if matches!(item, gix::status::Item::IndexWorktree(_)) {
                ensure!(
                    !changed
                        .iter()
                        .any(|path| overlaps(item.location(), path.as_bstr())),
                    "Checkout would overwrite working-tree changes at {}",
                    item.location()
                );
            }
        }
        let removed: BTreeSet<_> = changed
            .iter()
            .filter(|path| staged.contains_key(*path))
            .cloned()
            .collect();
        let snapshots = removed
            .iter()
            .map(|path| {
                let relative = gix::path::from_bstr(path)?;
                Ok((
                    path.clone(),
                    fingerprint(&self.repository.root.join(relative), repo.object_hash())?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        for path in &changed {
            if after.contains_key(path) {
                validate_destination(
                    &self.repository.root,
                    Path::new(&*gix::path::from_bstr(path)?),
                    &removed,
                )?;
            }
        }
        self.ensure_current()?;
        let lock = gix::lock::File::acquire_to_update_resource(
            repo.index_path(),
            gix::lock::acquire::Fail::Immediately,
            None,
            0,
        )?;
        self.ensure_current()?;
        let fail = gix::lock::acquire::Fail::Immediately;
        let transaction = repo.refs.transaction().prepare(edits, fail, fail)?;
        if fast_forward.is_some() {
            // HEAD may have switched to another branch at the same commit before locking.
            ensure!(
                repo.head_name()?.as_ref() == Some(&refname),
                "Git branch changed during pull; retry pull"
            );
        } else {
            branch::ensure_branch_unused(&repo, refname.as_ref())?;
        }
        let scratch = tempfile::Builder::new()
            .prefix(".mitos-checkout-")
            .tempdir_in(&self.repository.root)?;
        let prepared = scratch.path().join("new");
        let backup = scratch.path().join("old");
        std::fs::create_dir(&prepared)?;
        std::fs::create_dir(&backup)?;
        // Keep all target .gitattributes entries so checkout uses the destination's filters.
        target.remove_entries(|_, path, _| {
            !changed.contains(path)
                && path.rsplit(|byte| *byte == b'/').next() != Some(b".gitattributes")
        });
        let mut options =
            repo.checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)?;
        options.destination_is_initially_empty = true;
        let outcome = gix::worktree::state::checkout(
            &mut target,
            &prepared,
            repo.objects.clone().into_arc()?,
            &gix::progress::Discard,
            &gix::progress::Discard,
            &AtomicBool::new(false),
            options,
        )?;
        ensure!(
            outcome.errors.is_empty() && outcome.collisions.is_empty(),
            "Checkout reported errors or path collisions"
        );
        for (path, expected) in &snapshots {
            ensure!(
                &fingerprint(
                    &self.repository.root.join(gix::path::from_bstr(path)?),
                    repo.object_hash()
                )? == expected,
                "Working-tree file changed during checkout preparation: {path}"
            );
        }
        // Filters may take time or run helpers. Recheck destinations before moving
        // originals, so files created while filtering are kept in place.
        for path in &changed {
            if after.contains_key(path) {
                validate_destination(
                    &self.repository.root,
                    &gix::path::from_bstr(path)?,
                    &removed,
                )?;
            }
        }
        let mut next = (*original).clone();
        next.remove_entries(|_, path, _| changed.contains(path));
        for path in &changed {
            if let Some(entry) = target.entry_by_path(path.as_bstr()) {
                next.dangerously_push_entry(
                    Default::default(),
                    entry.id,
                    entry.flags,
                    entry.mode,
                    path.as_bstr(),
                );
            }
        }
        next.sort_entries();
        next.remove_tree();
        let mut output = std::io::BufWriter::new(lock);
        next.write_to(&mut output, Default::default())?;
        let lock = output.into_inner().map_err(|error| error.into_error())?;
        // Preserve originals until both index and HEAD have been committed. An I/O failure
        // retains this directory for recovery instead of discarding the saved files.
        if let Ok(index) = std::fs::read(repo.index_path()) {
            std::fs::write(scratch.path().join("index"), index)?;
        }
        let result = (|| -> Result<()> {
            for path in &removed {
                let relative = gix::path::from_bstr(path)?;
                let destination = backup.join(&relative);
                std::fs::create_dir_all(destination.parent().unwrap())?;
                std::fs::rename(self.repository.root.join(&relative), &destination)?;
                ensure!(
                    fingerprint(&destination, repo.object_hash())? == snapshots[path],
                    "Working-tree file changed during checkout: {path}"
                );
            }
            for path in &changed {
                if !after.contains_key(path) {
                    continue;
                }
                let relative = gix::path::from_bstr(path)?;
                let destination = self.repository.root.join(&relative);
                validate_destination(&self.repository.root, &relative, &BTreeSet::new())?;
                if destination.is_dir() {
                    remove_empty_directories(&destination)?;
                }
                std::fs::create_dir_all(destination.parent().unwrap())?;
                let source = prepared.join(&relative);
                if std::fs::symlink_metadata(&source)?.is_symlink() {
                    gix::fs::symlink::create(&std::fs::read_link(&source)?, &destination)?;
                } else {
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&destination)?;
                    std::io::copy(&mut std::fs::File::open(&source)?, &mut file)?;
                    file.set_permissions(std::fs::metadata(source)?.permissions())?;
                }
            }
            lock.commit()?;
            transaction.commit(repo.committer().transpose()?)?;
            Ok(())
        })();
        if let Err(error) = result {
            let recovery = scratch.keep();
            bail!(
                "Checkout could not finish: {error}. Original files and index are saved at {}",
                recovery.display()
            );
        }
        Ok(true)
    }
}

fn overlaps(left: &gix::bstr::BStr, right: &gix::bstr::BStr) -> bool {
    left == right
        || left
            .strip_prefix(right.as_bytes())
            .is_some_and(|rest| rest.starts_with(b"/"))
        || right
            .strip_prefix(left.as_bytes())
            .is_some_and(|rest| rest.starts_with(b"/"))
}

fn validate_destination(root: &Path, relative: &Path, removed: &BTreeSet<BString>) -> Result<()> {
    for ancestor in relative
        .ancestors()
        .take_while(|path| !path.as_os_str().is_empty())
    {
        let path = root.join(ancestor);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_dir() => {
                let key = gix::path::to_unix_separators_on_windows(gix::path::into_bstr(ancestor)?);
                ensure!(
                    removed.contains(key.as_ref()),
                    "Checkout would overwrite an untracked or ignored path: {}",
                    path.display()
                );
            }
            Ok(_) if ancestor == relative => validate_directory_contents(root, &path, removed)?,
            Ok(_) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.kind() == std::io::ErrorKind::NotADirectory => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn validate_directory_contents(
    root: &Path,
    directory: &Path,
    removed: &BTreeSet<BString>,
) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            validate_directory_contents(root, &entry.path(), removed)?;
        } else {
            let key = gix::path::to_unix_separators_on_windows(gix::path::into_bstr(
                entry.path().strip_prefix(root)?,
            )?)
            .into_owned();
            ensure!(
                removed.contains(&key),
                "Checkout would overwrite an untracked or ignored path: {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn remove_empty_directories(directory: &Path) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_empty_directories(&entry.path())?;
        }
    }
    std::fs::remove_dir(directory)?;
    Ok(())
}

fn fingerprint(
    path: &Path,
    hash: gix::hash::Kind,
) -> Result<(gix::ObjectId, std::fs::Permissions)> {
    let metadata = std::fs::symlink_metadata(path)?;
    let bytes = if metadata.is_symlink() {
        gix::path::into_bstr(std::fs::read_link(path)?)?
            .into_owned()
            .into()
    } else {
        std::fs::read(path)?
    };
    Ok((
        gix::objs::compute_hash(hash, gix::objs::Kind::Blob, &bytes)?,
        metadata.permissions(),
    ))
}

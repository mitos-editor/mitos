//! Create and safely delete local branches and their tracking settings.
use super::*;
use gix::bstr::{BString, ByteSlice};

struct Tracking {
    remote: BString,
    merges: Vec<BString>,
    rebase: bool,
}

impl Tracking {
    fn read(repo: &gix::Repository) -> Result<Option<Self>> {
        let config = repo.config_snapshot();
        let rebase = config.string("branch.autoSetupRebase");
        let rebase = rebase.as_deref().unwrap_or("never".into());
        ensure!(
            matches!(
                rebase.as_bytes(),
                b"never" | b"local" | b"remote" | b"always"
            ),
            "Invalid branch.autoSetupRebase setting"
        );
        let mode = config.string("branch.autoSetupMerge");
        let mode = mode.as_deref().unwrap_or("true".into());
        let (always, inherit) = match mode.as_bytes() {
            b"always" => (true, false),
            b"inherit" => (false, true),
            b"simple" => (false, false),
            _ => {
                config
                    .plumbing()
                    .boolean("branch.autoSetupMerge")
                    .transpose()?;
                (false, false)
            }
        };
        let Some(source) = repo
            .head_name()?
            .filter(|name| name.as_bstr().starts_with(b"refs/heads/"))
        else {
            return Ok(None);
        };
        let (remote, merges) = if always {
            (BString::from("."), vec![source.as_bstr().to_owned()])
        } else if inherit {
            let raw = config.plumbing();
            let Some(remote) = raw.string_by("branch", Some(source.shorten()), "remote") else {
                return Ok(None);
            };
            let Some(merges) = raw.strings_by("branch", Some(source.shorten()), "merge") else {
                return Ok(None);
            };
            (
                remote.into_owned(),
                merges.into_iter().map(|value| value.into_owned()).collect(),
            )
        } else {
            return Ok(None);
        };
        let local = remote == ".";
        Ok(Some(Self {
            remote,
            merges,
            rebase: rebase == "always"
                || (local && rebase == "local")
                || (!local && rebase == "remote"),
        }))
    }
}

impl PreparedOperation {
    pub(super) fn create_branch(&self, name: &str) -> Result<()> {
        let mut repo = self.repository.open_read(true)?;
        repo.committer_or_set_generic_fallback()?;
        let reference = format!("refs/heads/{name}");
        ensure!(
            repo.try_find_reference(reference.as_str())?.is_none(),
            "Branch already exists"
        );
        let head = repo.head_commit()?.id;
        let tracking = Tracking::read(&repo)?;
        let config_lock = if let Some(tracking) = tracking {
            let path = repo.common_dir().join("config");
            let lock = gix::lock::File::acquire_to_update_resource(
                &path,
                gix::lock::acquire::Fail::Immediately,
                None,
            )?;
            self.ensure_current()?;
            let contents = std::fs::read(&path)?;
            let mut config = gix::config::File::try_from(contents.as_bstr())?;
            let subsection = name.as_bytes().as_bstr();
            for key in ["remote", "merge"] {
                if let Ok(mut values) = config.raw_values_mut_by("branch", Some(subsection), key) {
                    values.delete_all();
                }
            }
            let mut section = config.new_section("branch", Some(subsection.into()))?;
            section.push("remote".try_into()?, Some(tracking.remote.as_bstr()));
            for merge in &tracking.merges {
                section.push("merge".try_into()?, Some(merge.as_bstr()));
            }
            if tracking.rebase {
                section.set("rebase".try_into()?, "true".into());
            }
            let mut out = std::io::BufWriter::new(lock);
            config.write_to(&mut out)?;
            Some(out.into_inner().map_err(|error| error.into_error())?)
        } else {
            None
        };
        self.ensure_current()?;
        repo.reference(
            reference,
            head,
            // MustNotExist accepts an existing ref at the same target. Comparing
            // against a null ID also rejects that case under the reference lock.
            gix::refs::transaction::PreviousValue::ExistingMustMatch(
                repo.object_hash().null().into(),
            ),
            "branch: Created from HEAD",
        )?;
        if let Some(lock) = config_lock {
            lock.commit()
                .context("Branch created, but its tracking settings could not be saved")?;
        }
        Ok(())
    }
}

impl PreparedOperation {
    pub(super) fn delete_branch(&self, name: &str) -> Result<()> {
        let repo = self.repository.open_read(true)?;
        let refname = format!("refs/heads/{name}");
        let reference = repo.find_reference(refname.as_str())?;
        // Keep the original target: deletion must fail if another process moves the ref.
        let tip = reference
            .try_id()
            .context("Choose a direct local branch")?
            .detach();
        repo.find_commit(tip)?;
        let upstream = repository::upstream_reference(&repo, reference.name())?
            .map(|name| repo.try_find_reference(name.as_ref()))
            .transpose()?
            .flatten();
        let base = match upstream {
            Some(mut upstream) => upstream.peel_to_commit()?.id,
            None => repo.head_commit()?.id,
        };
        ensure!(
            repo.merge_base(tip, base)?.detach() == tip,
            "Branch is not fully merged into its upstream or HEAD"
        );
        ensure_branch_unused(&repo, reference.name())?;

        // Lock configuration before deleting the ref so concurrent writers cannot lose settings.
        let path = repo.common_dir().join("config");
        let lock = gix::lock::File::acquire_to_update_resource(
            &path,
            gix::lock::acquire::Fail::Immediately,
            None,
        )?;
        self.ensure_current()?;
        let contents = std::fs::read(&path)?;
        let mut config = gix::config::File::try_from(contents.as_bstr())?;
        while config
            .remove_section("branch", Some(name.as_bytes().as_bstr()))
            .is_some()
        {}
        let mut out = std::io::BufWriter::new(lock);
        config.write_to(&mut out)?;
        let lock = out.into_inner().map_err(|error| error.into_error())?;
        self.ensure_current()?;
        ensure_branch_unused(&repo, reference.name())?;
        reference.delete()?;
        lock.commit()
            .context("Branch deleted, but its tracking settings could not be removed")?;
        Ok(())
    }
}

fn ensure_branch_unused(repo: &gix::Repository, reference: &gix::refs::FullNameRef) -> Result<()> {
    ensure_worktree_not_using_branch(&repo.main_repo()?, reference)?;
    for worktree in repo.worktrees()? {
        ensure_worktree_not_using_branch(
            &worktree.into_repo_with_possibly_inaccessible_worktree()?,
            reference,
        )?;
    }
    Ok(())
}

fn ensure_worktree_not_using_branch(
    repo: &gix::Repository,
    reference: &gix::refs::FullNameRef,
) -> Result<()> {
    if repo.is_bare() {
        return Ok(());
    }
    ensure!(
        repo.head_name()?
            .as_ref()
            .is_none_or(|head| head.as_ref() != reference),
        "Cannot delete a branch checked out in a worktree"
    );
    // Rebase and bisect detach HEAD but still reserve branches. Rebase --update-refs
    // also reserves additional branches listed in its state file.
    for file in [
        "rebase-merge/head-name",
        "rebase-apply/head-name",
        "BISECT_START",
        "rebase-merge/update-refs",
    ] {
        let contents = match std::fs::read(repo.git_dir().join(file)) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        ensure!(
            !contents.lines().any(|line| {
                let line = line.trim();
                line == reference.as_bstr()
                    || (file == "BISECT_START" && line == reference.shorten())
            }),
            "Cannot delete a branch used by a rebase or bisect in a worktree"
        );
    }
    Ok(())
}

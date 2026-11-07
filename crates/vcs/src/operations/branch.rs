//! Create local branches and their tracking settings without changing HEAD.
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

//! Stage selected files through gix's attribute and clean-filter pipeline.
use super::*;
use gix::bstr::ByteSlice;

impl PreparedOperation {
    pub(super) fn stage(&self) -> Result<bool> {
        let Operation::File { paths, .. } = &self.operation else {
            unreachable!()
        };
        let repo = self.repository.open(true)?;
        let lock = gix::lock::File::acquire_to_update_resource(
            repo.index_path(),
            gix::lock::acquire::Fail::Immediately,
            None,
            0,
        )?;
        self.ensure_current()?;
        let mut index = (*repo.index_or_empty()?).clone();
        if !index::can_write(&repo, &index) {
            return Ok(false);
        }
        let config = repo.config_snapshot();
        let ignore_case = config.boolean("core.ignoreCase").unwrap_or(false);
        let filemode = config.boolean("core.fileMode").unwrap_or(true);
        let symlinks = config.boolean("core.symlinks").unwrap_or(true);
        let selected = paths
            .iter()
            .map(|path| {
                Ok(
                    gix::path::to_unix_separators_on_windows(gix::path::into_bstr(path)?)
                        .into_owned(),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        // Leave directory/file transitions, case aliases, and hidden entries to Git.
        for path in &selected {
            if path.split(|byte| *byte == b'/').any(|component| {
                gix::validate::path::component(component.as_bstr(), None, Default::default())
                    .is_err()
            }) {
                return Ok(false);
            }
            for entry in index.entries() {
                let existing = entry.path(&index);
                if existing == path.as_bstr() {
                    if entry.flags.intersects(
                        gix::index::entry::Flags::ASSUME_VALID
                            | gix::index::entry::Flags::SKIP_WORKTREE,
                    ) || (!symlinks && entry.mode == gix::index::entry::Mode::SYMLINK)
                    {
                        return Ok(false);
                    }
                } else if (ignore_case
                    && existing
                        .split(|byte| *byte == b'/')
                        .zip(path.split(|byte| *byte == b'/'))
                        .take_while(|(left, right)| left.eq_ignore_ascii_case(right))
                        .any(|(left, right)| left != right))
                    || existing
                        .strip_prefix(path.as_slice())
                        .is_some_and(|rest| rest.starts_with(b"/"))
                    || path
                        .strip_prefix(existing.as_bytes())
                        .is_some_and(|rest| rest.starts_with(b"/"))
                {
                    return Ok(false);
                }
            }
            let metadata = match std::fs::symlink_metadata(
                self.repository.root.join(gix::path::from_bstr(path)?),
            ) {
                Ok(metadata) => Some(metadata),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            if let Some(metadata) = metadata {
                if metadata.is_dir() || (!symlinks && metadata.is_symlink()) {
                    return Ok(false);
                }
                if metadata.is_symlink()
                    && path
                        .rsplit(|byte| *byte == b'/')
                        .next()
                        .is_some_and(|name| name.eq_ignore_ascii_case(b".gitmodules"))
                {
                    return Ok(false);
                }
                ensure!(
                    metadata.is_file() || metadata.is_symlink(),
                    "Cannot stage this file type"
                );
            }
        }
        let attributes = repo.attributes_only(
            &index,
            gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
        )?;
        let mut filters = gix::filter::Pipeline::new(&repo, attributes.detach())?;
        let mut excludes = repo.excludes(
            &index,
            None,
            gix::worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
        )?;
        let mut replacements = Vec::new();
        for path in selected {
            let existing = index.entry_by_path(path.as_bstr());
            if existing.is_none() {
                ensure!(
                    !excludes.at_entry(path.as_bstr(), None)?.is_excluded(),
                    "Selected file is ignored"
                );
            }
            let entry = filters
                .worktree_file_to_object(path.as_bstr(), &index)?
                .map(|(id, kind, _)| {
                    let mut mode: gix::index::entry::Mode = kind.into();
                    if !filemode
                        && matches!(
                            kind,
                            gix::objs::tree::EntryKind::Blob
                                | gix::objs::tree::EntryKind::BlobExecutable
                        )
                    {
                        mode = existing
                            .filter(|entry| {
                                matches!(
                                    entry.mode,
                                    gix::index::entry::Mode::FILE
                                        | gix::index::entry::Mode::FILE_EXECUTABLE
                                )
                            })
                            .map_or(gix::index::entry::Mode::FILE, |entry| entry.mode);
                    }
                    (id, mode)
                });
            ensure!(
                !entry.as_ref().is_some_and(|(_, mode)| mode.is_submodule()),
                "Selected file became a directory; refresh and try again"
            );
            replacements.push((path, entry));
        }
        index.remove_entries(|_, path, _| {
            replacements
                .iter()
                .any(|(selected, _)| selected.as_bstr() == path)
        });
        for (path, entry) in replacements {
            if let Some((id, mode)) = entry {
                index.dangerously_push_entry(
                    Default::default(),
                    id,
                    gix::index::entry::Flags::empty(),
                    mode,
                    path.as_bstr(),
                );
            }
        }
        index.sort_entries();
        self.ensure_current()?;
        index::write(&mut index, lock)?;
        Ok(true)
    }
}

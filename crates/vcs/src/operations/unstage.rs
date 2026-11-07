//! Restore selected index entries from HEAD without changing working files.
use super::*;
use gix::bstr::ByteSlice;

impl PreparedOperation {
    /// Return false for index shapes the native writer cannot round-trip yet.
    pub(super) fn unstage(&self) -> Result<bool> {
        let Operation::File { paths, unborn, .. } = &self.operation else {
            unreachable!()
        };
        let repo = self.repository.open_read(true)?;
        let lock = gix::lock::File::acquire_to_update_resource(
            repo.index_path(),
            gix::lock::acquire::Fail::Immediately,
            None,
        )?;
        self.ensure_current()?;
        let mut index = repo.open_index()?;
        // gix cannot serialize resolve-undo information. Keep sparse checkout
        // and conflict-index semantics with Git as well.
        if index.is_sparse()
            || repo
                .config_snapshot()
                .boolean("core.sparseCheckout")
                .unwrap_or(false)
            || index.resolve_undo().is_some()
            || index
                .entries()
                .iter()
                .any(|entry| entry.stage() != gix::index::entry::Stage::Unconflicted)
        {
            return Ok(false);
        }
        let tree = if *unborn {
            None
        } else {
            Some(repo.head_commit()?.tree()?)
        };
        let mut replacements = Vec::new();
        for path in paths {
            let bytes = gix::path::to_unix_separators_on_windows(gix::path::try_into_bstr(path)?)
                .into_owned();
            let entry = tree
                .as_ref()
                .map(|tree| tree.lookup_entry_by_path(path))
                .transpose()?
                .flatten();
            if entry.as_ref().is_some_and(|entry| entry.mode().is_tree()) {
                return Ok(false);
            }
            let flags = index
                .entry_by_path(bytes.as_bstr())
                .map_or(gix::index::entry::Flags::empty(), |entry| {
                    entry.flags & gix::index::entry::Flags::SKIP_WORKTREE
                });
            replacements.push((
                bytes,
                entry.map(|entry| (entry.object_id(), entry.mode())),
                flags,
            ));
        }
        index.remove_entries(|_, path, _| {
            replacements
                .iter()
                .any(|(selected, _, _)| selected.as_bstr() == path)
        });
        for (path, entry, flags) in replacements {
            if let Some((id, mode)) = entry {
                index.dangerously_push_entry(
                    Default::default(),
                    id,
                    flags,
                    mode.into(),
                    path.as_bstr(),
                );
            }
        }
        index.sort_entries();
        index.remove_tree();
        let mut out = std::io::BufWriter::new(lock);
        index.write_to(&mut out, Default::default())?;
        out.into_inner()
            .map_err(|error| error.into_error())?
            .commit()?;
        Ok(true)
    }
}

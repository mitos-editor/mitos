//! Restore selected index entries from HEAD without changing working files.
use super::*;
use gix::bstr::{BString, ByteSlice};
use std::collections::BTreeMap;

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
            0,
        )?;
        self.ensure_current()?;
        let mut index = repo.open_index()?;
        if !index::can_write(&repo, &index) {
            return Ok(false);
        }
        let tree = if *unborn {
            None
        } else {
            Some(repo.head_commit()?.tree()?)
        };
        let mut replacements = BTreeMap::new();
        let mut selected = Vec::new();
        for path in paths {
            let bytes =
                gix::path::to_unix_separators_on_windows(gix::path::into_bstr(path)?).into_owned();
            let entry = tree
                .as_ref()
                .map(|tree| tree.lookup_entry_by_path(path))
                .transpose()?
                .flatten();
            selected.push(bytes.clone());
            if let Some(entry) = entry {
                if entry.mode().is_tree() {
                    let subtree = repo.index_from_tree(&entry.object_id())?;
                    for entry in subtree.entries() {
                        let mut full = bytes.clone();
                        full.push(b'/');
                        full.extend_from_slice(entry.path(&subtree));
                        replacements.insert(full, (entry.id, entry.mode));
                    }
                } else {
                    replacements.insert(bytes, (entry.object_id(), entry.mode().into()));
                }
            }
        }
        let replacements: Vec<_> = replacements
            .into_iter()
            .map(|(path, (id, mode))| {
                let flags = index
                    .entry_by_path(path.as_bstr())
                    .map_or(gix::index::entry::Flags::empty(), |entry| {
                        entry.flags & gix::index::entry::Flags::SKIP_WORKTREE
                    });
                (path, id, mode, flags)
            })
            .collect();
        index.remove_entries(|_, path, _| {
            selected.iter().any(|selected: &BString| {
                path == selected.as_bstr()
                    || path
                        .strip_prefix(selected.as_slice())
                        .is_some_and(|rest| rest.starts_with(b"/"))
            })
        });
        for (path, id, mode, flags) in replacements {
            index.dangerously_push_entry(Default::default(), id, flags, mode, path.as_bstr());
        }
        index.sort_entries();
        index::write(&mut index, lock)?;
        Ok(true)
    }
}

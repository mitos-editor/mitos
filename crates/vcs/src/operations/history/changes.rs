//! Tree comparisons use raw repository blobs, never worktree filters or external diffs.
use super::*;
use gix::object::tree::diff::Change;
use std::ops::ControlFlow;

pub(super) fn between(
    before: &gix::Tree<'_>,
    after: &gix::Tree<'_>,
    canceled: impl Fn() -> bool,
) -> Result<Vec<HistoryFile>> {
    let mut files = Vec::new();
    let mut changes = before.changes()?;
    changes.options(|options| {
        options
            .track_path()
            .track_rewrites(Some(gix::diff::Rewrites::default()));
    });
    changes.for_each_to_obtain_tree(after, |change| -> gix::Result<_> {
        if canceled() {
            return Ok(ControlFlow::Break(()));
        }
        let path = |location: &gix::bstr::BStr| -> gix::Result<PathBuf> {
            let path = gix::path::from_bstr(location)?.into_owned();
            validate_path(&path)
                .map_err(|error| gix::Error::from_boxed(error.into_boxed_dyn_error()))?;
            Ok(path)
        };
        let file = match change {
            Change::Addition {
                location,
                entry_mode,
                ..
            } if !entry_mode.is_tree() => HistoryFile {
                before: None,
                after: Some(path(location)?),
                status: "A".into(),
            },
            Change::Deletion {
                location,
                entry_mode,
                ..
            } if !entry_mode.is_tree() => HistoryFile {
                before: Some(path(location)?),
                after: None,
                status: "D".into(),
            },
            Change::Modification {
                location,
                previous_entry_mode,
                entry_mode,
                ..
            } if !entry_mode.is_tree() => {
                let path = path(location)?;
                HistoryFile {
                    before: Some(path.clone()),
                    after: Some(path),
                    status: if previous_entry_mode.kind() == entry_mode.kind()
                        || (previous_entry_mode.is_blob() && entry_mode.is_blob())
                    {
                        "M"
                    } else {
                        "T"
                    }
                    .into(),
                }
            }
            Change::Rewrite {
                source_location,
                location,
                copy,
                diff,
                ..
            } => HistoryFile {
                before: Some(path(source_location)?),
                after: Some(path(location)?),
                status: format!(
                    "{}{:03}",
                    if copy { "C" } else { "R" },
                    diff.map_or(100, |stats| (stats.similarity * 100.0) as u32)
                ),
            },
            _ => return Ok(ControlFlow::Continue(())),
        };
        files.push(file);
        gix::error::ensure!(
            files.len() <= 20_000,
            "Commit contains too many files to display (limit 20000)"
        );
        Ok(ControlFlow::Continue(()))
    })?;
    ensure!(!canceled(), "History read canceled");
    files.sort_by(|a, b| a.path().cmp(b.path()));
    Ok(files)
}

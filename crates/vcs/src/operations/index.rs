//! Shared limits and locking for native index edits.
use super::*;

pub(super) fn can_write(repo: &gix::Repository, index: &gix::index::File) -> bool {
    // gix cannot serialize resolve-undo information. Preserve sparse checkout
    // and conflict-index semantics with Git as well.
    !index.is_sparse()
        && !repo
            .config_snapshot()
            .boolean("core.sparseCheckout")
            .unwrap_or(false)
        && index.resolve_undo().is_none()
        && index
            .entries()
            .iter()
            .all(|entry| entry.stage() == gix::index::entry::Stage::Unconflicted)
}

pub(super) fn write(index: &mut gix::index::File, lock: gix::lock::File) -> Result<()> {
    index.remove_tree();
    let mut out = std::io::BufWriter::new(lock);
    index.write_to(&mut out, Default::default())?;
    out.into_inner()
        .map_err(|error| error.into_error())?
        .commit()?;
    Ok(())
}

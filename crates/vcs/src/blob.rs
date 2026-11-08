//! Worktree conversion shared by gutter bases and historical comparisons.

use anyhow::{ensure, Result};
use std::{io::Read, path::Path};

/// Convert regular blob data with the already-opened repository's trust and filter policy.
/// Callers handle symlinks separately and may bound the expanded filter output.
pub(crate) fn to_worktree(
    repo: &gix::Repository,
    data: &[u8],
    relative_path: &Path,
    max_bytes: Option<usize>,
) -> Result<Vec<u8>> {
    let relative_path = gix::path::into_bstr(relative_path)?;
    let (mut pipeline, _) = repo.filter_pipeline(None)?;
    let mut outcome = pipeline.convert_to_worktree(
        data,
        relative_path.as_ref(),
        gix::filter::plumbing::pipeline::convert::to_worktree::Options {
            can_delay: gix::filter::plumbing::driver::apply::Delay::Forbid,
            ..Default::default()
        },
    )?;
    let mut bytes = Vec::new();
    if let Some(limit) = max_bytes {
        outcome
            .by_ref()
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= limit,
            "Filtered file too large (limit {limit} bytes)"
        );
    } else {
        outcome.read_to_end(&mut bytes)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanded_worktree_data_obeys_the_callers_limit() {
        let directory = crate::git::test::empty_git_repo();
        std::fs::write(
            directory.path().join(".gitattributes"),
            "*.txt text eol=crlf\n",
        )
        .unwrap();
        let repo = crate::git::open_repo(directory.path(), false)
            .unwrap()
            .to_thread_local();
        let path = Path::new("file.txt");
        assert!(to_worktree(&repo, b"a\n", path, Some(2)).is_err());
        assert_eq!(to_worktree(&repo, b"a\n", path, Some(3)).unwrap(), b"a\r\n");
        assert_eq!(to_worktree(&repo, b"a\n", path, None).unwrap(), b"a\r\n");
    }
}

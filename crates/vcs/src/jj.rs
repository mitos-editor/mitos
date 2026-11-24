//! Read the parent of a Jujutsu working-copy commit through the user's `jj`.

use std::{
    fs::{self, File},
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{ensure, Context, Result};
use workspace::{trust::TrustQuery, WorkspaceTrust};

use crate::{Comparison, ComparisonKind, DocumentVcs, RepositoryWatches, MAX_REVIEW_BYTES};

fn workspace_root<'a>(path: &'a Path, trust: &WorkspaceTrust) -> Result<&'a Path> {
    let root = path
        .ancestors()
        .find(|path| {
            [".jj", ".git", ".svn"]
                .iter()
                .any(|marker| path.join(marker).exists())
        })
        .filter(|path| path.join(".jj").is_dir())
        .context("not in a Jujutsu workspace")?;
    ensure!(
        trust.query(root, TrustQuery::Git).is_trusted(),
        "Jujutsu repositories require a trusted workspace"
    );
    Ok(root)
}

pub(super) fn repository_watches(path: &Path, trust: &WorkspaceTrust) -> Result<RepositoryWatches> {
    let root = workspace_root(path, trust)?;
    Ok(RepositoryWatches {
        paths: vec![root.join(".jj/working_copy/checkout")],
        files: Vec::new(),
    })
}

pub(super) fn inspect_file(
    file: &Path,
    trust: &WorkspaceTrust,
    read_diff: bool,
) -> Result<DocumentVcs> {
    if !read_diff {
        return Ok(DocumentVcs::default());
    }
    Ok(DocumentVcs {
        diff_base: Some(diff_base(file, trust)?),
        head_name: None,
    })
}

fn diff_base(file: &Path, trust: &WorkspaceTrust) -> Result<Vec<u8>> {
    let root = workspace_root(file, trust)?;
    let relative = file
        .strip_prefix(root)?
        .to_str()
        .context("Jujutsu paths must be UTF-8")?;
    #[cfg(windows)]
    let relative = relative.replace('\\', "/");

    // An exact matcher in a compound expression yields empty output for absent files.
    // An explicit template prefixes the bytes with the entry type and overrides user templates.
    let fileset = format!("root-file:{relative:?} & all()");
    let jj = stdx::env::which("jj")?;
    // Pin the operation and explicitly suppress working-copy snapshots.
    let output = Command::new(jj)
        .current_dir(root)
        .stdin(Stdio::null())
        .args([
            "--at-operation=@",
            "--ignore-working-copy",
            "--color=never",
            "--no-pager",
            "--quiet",
        ])
        .args([
            "file",
            "show",
            "-r",
            "@-",
            "-T",
            r#"file_type ++ "\0""#,
            "--",
        ])
        .arg(fileset)
        .output()
        .context("failed to run `jj`")?;
    ensure!(
        output.status.success(),
        "`jj file show` failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let mut bytes = output.stdout;
    if !bytes.is_empty() {
        ensure!(
            bytes.starts_with(b"file\0"),
            "Jujutsu diff base is not a regular file"
        );
        bytes.drain(..b"file\0".len());
    }
    Ok(bytes)
}

pub(super) fn read_comparison(
    path: &Path,
    kind: ComparisonKind,
    trust: &WorkspaceTrust,
) -> Result<Comparison> {
    ensure!(
        matches!(kind, ComparisonKind::Head | ComparisonKind::HeadToWorktree),
        "Jujutsu comparisons use the working-copy parent"
    );
    let before = diff_base(path, trust)?;
    ensure!(
        before.len() <= MAX_REVIEW_BYTES,
        "File too large to review (limit 10 MiB)"
    );
    let after = if kind == ComparisonKind::HeadToWorktree {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                ensure!(metadata.is_file(), "Jujutsu review requires a regular file");
                crate::review::bounded_read(&mut File::open(path)?)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        }
    } else {
        Vec::new()
    };
    Ok(Comparison { before, after })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiffProviderRegistry, FileChange, FileStatus};

    #[test]
    fn skips_a_parent_jujutsu_workspace_for_a_nested_git_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let nested = root.join("nested");
        fs::create_dir(root.join(".jj")).unwrap();
        fs::create_dir_all(nested.join(".git")).unwrap();

        let trust = WorkspaceTrust::fully_trusted(root.join(".jj/test-trust"));
        assert!(workspace_root(&nested.join("file.txt"), &trust).is_err());
    }

    #[test]
    fn reads_the_parent_for_gutter_markers_and_file_review() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let jj = |args: &[&str]| {
            Command::new("jj")
                .current_dir(root)
                .args([
                    "--config=user.name=Test",
                    "--config=user.email=test@example.invalid",
                ])
                .args(args)
                .output()
        };
        match jj(&["git", "init", "--no-colocate"]) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            result => assert!(result.unwrap().status.success()),
        }
        fs::create_dir(root.join("sub dir")).unwrap();
        let path = root.join("sub dir/notes $left.txt");
        fs::write(&path, b"parent\n").unwrap();
        assert!(jj(&["commit", "-m", "parent"]).unwrap().status.success());
        fs::write(&path, b"worktree\n").unwrap();

        let trust = WorkspaceTrust::fully_trusted(root.join(".jj/test-trust"));
        let providers = DiffProviderRegistry::with_workspace_trust(trust);
        let base = |path: &Path| providers.inspect_file(path, false, true).diff_base.unwrap();
        let watches = providers.repository_watches(root, false);
        assert_eq!(watches.paths, [root.join(".jj/working_copy/checkout")]);
        let marker = || fs::read(&watches.paths[0]).unwrap();
        let before = marker();
        assert_eq!(base(&path), b"parent\n");
        assert_eq!(base(&root.join("new.txt")), b"");
        assert_eq!(marker(), before);

        let status = FileStatus {
            staged: None,
            unstaged: Some(FileChange::Modified { path: path.clone() }),
        };
        let comparison = providers
            .read_current_comparison(&status, ComparisonKind::HeadToWorktree, false)
            .unwrap();
        assert_eq!(comparison.before, b"parent\n");
        assert_eq!(comparison.after, b"worktree\n");

        assert!(jj(&["commit", "-m", "next"]).unwrap().status.success());
        assert_ne!(marker(), before);
        assert_eq!(base(&path), b"worktree\n");

        assert!(jj(&["new", "@", "@-"]).unwrap().status.success());
        assert!(providers
            .inspect_file(&path, false, true)
            .diff_base
            .is_none());
    }
}

//! Jujutsu support through the `jj` executable.
//!
//! `jj-lib` cannot load storage backends newer than itself or private to a custom build, so
//! only the user's own binary reads every repository they work in.
//!
//! Jujutsu has no reduced-trust mode: repository configuration can name external programs
//! such as diff formatters. Every entry point therefore requires a trusted workspace, and an
//! untrusted colocated repository falls back to the Git provider.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

use anyhow::{ensure, Context, Result};
use arc_swap::ArcSwap;
use workspace::{trust::TrustQuery, WorkspaceTrust};

use crate::{
    ChangedFileScope, Comparison, ComparisonKind, DocumentVcs, FileChange, FileStatus,
    RepositoryWatches,
};

/// Root of the workspace containing `path`.
fn workspace_root<'a>(path: &'a Path, trust: &WorkspaceTrust) -> Result<&'a Path> {
    let root = path
        .ancestors()
        .find(|path| path.join(".jj").is_dir())
        .context("not in a Jujutsu workspace")?;
    ensure!(
        trust.query(root, TrustQuery::Git).is_trusted(),
        "Jujutsu repositories require a trusted workspace"
    );
    Ok(root)
}

/// Rewritten by every operation, including revision switches made outside the editor.
fn checkout_marker(root: &Path) -> PathBuf {
    root.join(".jj/working_copy/checkout")
}

/// Run from the workspace root so that templates print root-relative paths.
fn jj(root: &Path) -> Command {
    let mut command = Command::new("jj");
    command
        .current_dir(root)
        .stdin(Stdio::null())
        .args(["--color", "never", "--no-pager", "--quiet"])
        // Snapshot the working copy for current answers without recording an operation,
        // which would rewrite the watched checkout marker and trigger another refresh.
        .arg("--no-integrate-operation");
    command
}

fn run(command: &mut Command) -> Result<Vec<u8>> {
    let output = command.output().context("failed to run `jj`")?;
    ensure!(
        output.status.success(),
        "`jj` failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

pub(super) fn inspect_file(
    file: &Path,
    trust: &WorkspaceTrust,
    read_diff: bool,
) -> Result<DocumentVcs> {
    let root = workspace_root(file, trust)?;
    Ok(DocumentVcs {
        diff_base: read_diff
            .then(|| {
                diff_base(root, file)
                    .inspect_err(|error| log::debug!("Diff base for {}: {error:#}", file.display()))
                    .ok()
            })
            .flatten(),
        head_name: head_name(root)
            .inspect_err(|error| log::debug!("Head name for {}: {error:#}", file.display()))
            .ok(),
    })
}

pub(super) fn repository_watches(path: &Path, trust: &WorkspaceTrust) -> Result<RepositoryWatches> {
    Ok(RepositoryWatches {
        paths: vec![checkout_marker(workspace_root(path, trust)?)],
        files: Vec::new(),
    })
}

/// Content of `file` in the parents of the working-copy commit, merged when there are several.
fn diff_base(root: &Path, file: &Path) -> Result<Vec<u8>> {
    let relative = file
        .strip_prefix(root)?
        .to_str()
        .context("Jujutsu paths are UTF-8")?;
    let diff = |option: &str, value: &OsStr| {
        run(jj(root)
            .args(["diff", "--revision", "@", option])
            .arg(value)
            // A bare path is parsed as a fileset expression. Rust's string escapes are also
            // valid in fileset and TOML strings for any path without control characters.
            .args(["--", &format!("root-file:{relative:?}")]))
    };
    match diff("--template", "status_char".as_ref())?.first() {
        // Unchanged in the working copy.
        None => std::fs::read(file).context("read unchanged file as its own diff base"),
        Some(b'A' | b'C') => Ok(Vec::new()),
        // `jj file show -r @-` would fail on a merge. A diff formatter instead receives
        // the already merged parent tree, so print the file from there.
        Some(_) => {
            let left = Path::new("$left").join(relative);
            let print = if cfg!(windows) {
                r#""cmd", "/C", "type""#
            } else {
                r#""cat""#
            };
            diff(
                "--config",
                format!("ui.diff-formatter=[{print}, {left:?}]").as_ref(),
            )
        }
    }
}

/// Title of the working-copy commit, or else its bookmarks, or else what it sits on: the
/// bookmarks or title of its parent in brackets, or the number of parents it merges.
const HEAD_NAME: &str = r#"coalesce(
    truncate_end(40, description.first_line(), "…"),
    local_bookmarks.map(|bookmark| bookmark.name()).join(" "),
    if(parents.len() == 1,
        "[" ++ parents.map(|parent| coalesce(
            parent.local_bookmarks().map(|bookmark| bookmark.name()).join(" "),
            truncate_end(40, parent.description().first_line(), "…"),
            parent.change_id().shortest(8))).join("") ++ "]",
        "[merge of " ++ parents.len() ++ "]"))"#;

fn head_name(root: &Path) -> Result<Arc<ArcSwap<Box<str>>>> {
    let output = run(jj(root).args([
        "log",
        "--no-graph",
        "--revisions",
        "@",
        "--template",
        HEAD_NAME,
    ]))?;
    Ok(Arc::new(ArcSwap::from_pointee(
        std::str::from_utf8(&output)?.into(),
    )))
}

/// Jujutsu has no staging area: every change is reported as unstaged.
pub(super) fn for_each_file_status(
    scope: &ChangedFileScope,
    trust: &WorkspaceTrust,
    f: impl Fn(&Path, Result<FileStatus>) -> bool,
) -> Result<()> {
    let root = workspace_root(scope.path(), trust)?;
    // Fields end with NUL, which no path contains. A conflicted file is reported once,
    // as a conflict, even when the working copy also changes it.
    let template = r#"concat(
        conflicted_files.map(|file| concat("C\0", file.path().display(), "\0")).join(""),
        diff.files()
            .filter(|file| conflicted_files.all(|c| c.path().display() != file.path().display()))
            .map(|file| concat(
                "D\0",
                file.status(), "\0",
                file.source().path().display(), "\0",
                file.source().file_type(), "\0",
                file.target().path().display(), "\0",
                file.target().file_type(), "\0",
            ))
            .join(""),
    )"#;
    let output = run(jj(root).args(["show", "@", "--no-patch", "--template", template]))?;
    let mut fields = output
        .split(|byte| *byte == 0)
        .map(|field| std::str::from_utf8(field).unwrap_or_default());
    while let Some(kind) = fields.next() {
        let change = match kind {
            "C" => fields.next().map(|path| FileChange::Conflict {
                path: root.join(path),
            }),
            "D" => {
                let [Some(status), Some(source), Some(source_type), Some(target), Some(target_type)] =
                    std::array::from_fn(|_| fields.next())
                else {
                    break;
                };
                diff_entry(root, status, (source, source_type), (target, target_type))
            }
            _ => break,
        };
        let in_scope = |change: &FileChange| match scope {
            ChangedFileScope::Directory(directory) => change.path().starts_with(directory),
            ChangedFileScope::Repository(_) => true,
        };
        if let Some(change) = change.filter(in_scope)
            && !f(
                root,
                Ok(FileStatus {
                    staged: None,
                    unstaged: Some(change),
                }),
            )
        {
            break;
        }
    }
    Ok(())
}

fn diff_entry(
    root: &Path,
    status: &str,
    (source, source_type): (&str, &str),
    (target, target_type): (&str, &str),
) -> Option<FileChange> {
    let path = root.join(target);
    if target_type == "conflict" {
        return Some(FileChange::Conflict { path });
    }
    // An added or removed file has no type on its missing side. Trees and submodules
    // are not files.
    let is_file = |file_type| matches!(file_type, "" | "conflict" | "file" | "symlink");
    if !is_file(source_type) || !is_file(target_type) {
        return None;
    }
    Some(match status {
        // Every new file is tracked automatically, like an untracked file in Git.
        "added" | "copied" => FileChange::Untracked { path },
        "modified" => FileChange::Modified { path },
        "removed" => FileChange::Deleted { path },
        "renamed" => FileChange::Renamed {
            from_path: root.join(source),
            to_path: path,
        },
        _ => return None,
    })
}

/// The parents of the working-copy commit stand in for both HEAD and the index.
pub(super) fn read_comparison(
    status: &FileStatus,
    kind: ComparisonKind,
    trust: &WorkspaceTrust,
) -> Result<Comparison> {
    ensure!(
        kind != ComparisonKind::Staged,
        "Jujutsu has no staging area"
    );
    let path = status.path();
    let root = workspace_root(path, trust)?;
    let source = match status.change() {
        FileChange::Renamed { from_path, .. } => from_path,
        change => change.path(),
    };
    let before = diff_base(root, source)?;
    ensure!(
        before.len() <= crate::MAX_REVIEW_BYTES,
        "File too large to review (limit 10 MiB)"
    );
    let after = match std::fs::File::open(path) {
        Ok(mut file)
            if matches!(
                kind,
                ComparisonKind::Unstaged | ComparisonKind::HeadToWorktree
            ) =>
        {
            crate::review::bounded_read(&mut file)?
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => Vec::new(),
    };
    Ok(Comparison { before, after })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace whose parent commit has `kept.txt`, `gone.txt` and `old name.txt`.
    fn workspace() -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().unwrap();
        let jj = |args: &[&str]| {
            Command::new("jj")
                .current_dir(dir.path())
                .args(args)
                .output()
                .is_ok_and(|output| output.status.success())
        };
        // Not every machine running the tests has Jujutsu.
        if !jj(&["git", "init"]) {
            return None;
        }
        for name in ["kept.txt", "gone.txt", "old name.txt"] {
            std::fs::write(dir.path().join(name), format!("{name}\nbase\n")).unwrap();
        }
        assert!(jj(&["commit", "--message", "base"]));
        assert!(jj(&["bookmark", "create", "topic", "--revision", "@-"]));
        Some(dir)
    }

    #[test]
    fn reads_bases_statuses_and_comparisons_from_a_workspace() {
        let Some(dir) = workspace() else { return };
        let root = dir.path();
        let trust = WorkspaceTrust::fully_trusted(root.join(".jj/test-trust"));
        let path = |name: &str| root.join(name);
        let base = |name: &str| inspect_file(&path(name), &trust, true).unwrap().diff_base;

        assert_eq!(base("kept.txt").unwrap(), b"kept.txt\nbase\n");
        std::fs::write(path("kept.txt"), "changed\n").unwrap();
        std::fs::write(path("sub dir's \"new\".txt"), "new\n").unwrap();
        std::fs::remove_file(path("gone.txt")).unwrap();
        std::fs::rename(path("old name.txt"), path("new name.txt")).unwrap();
        assert_eq!(base("kept.txt").unwrap(), b"kept.txt\nbase\n");
        assert_eq!(base("sub dir's \"new\".txt").unwrap(), b"");

        let vcs = inspect_file(&path("kept.txt"), &trust, false).unwrap();
        assert!(vcs.diff_base.is_none());
        let head = vcs.head_name.unwrap().load().to_string();
        assert_eq!(head, "[topic]");

        let statuses = std::cell::RefCell::new(Vec::new());
        let scope = ChangedFileScope::Repository(root.into());
        for_each_file_status(&scope, &trust, |workspace, status| {
            assert_eq!(workspace, root);
            statuses.borrow_mut().push(status.unwrap());
            true
        })
        .unwrap();
        let statuses = statuses.into_inner();
        assert!(statuses.iter().all(|status| status.staged.is_none()));
        let mut changes: Vec<_> = statuses
            .iter()
            .map(|status| status.change().clone())
            .collect();
        changes.sort_by(|a, b| a.path().cmp(b.path()));
        assert_eq!(
            changes,
            [
                FileChange::Deleted {
                    path: path("gone.txt")
                },
                FileChange::Modified {
                    path: path("kept.txt")
                },
                FileChange::Renamed {
                    from_path: path("old name.txt"),
                    to_path: path("new name.txt"),
                },
                FileChange::Untracked {
                    path: path("sub dir's \"new\".txt")
                },
            ]
        );

        let comparison = |name: &str, kind| {
            let status = statuses
                .iter()
                .find(|status| status.path() == path(name))
                .unwrap();
            let comparison = read_comparison(status, kind, &trust).unwrap();
            (
                String::from_utf8(comparison.before).unwrap(),
                String::from_utf8(comparison.after).unwrap(),
            )
        };
        assert_eq!(
            comparison("kept.txt", ComparisonKind::Unstaged),
            ("kept.txt\nbase\n".into(), "changed\n".into())
        );
        assert_eq!(comparison("kept.txt", ComparisonKind::Head).1, "");
        assert_eq!(
            comparison("gone.txt", ComparisonKind::HeadToWorktree),
            ("gone.txt\nbase\n".into(), String::new())
        );
        assert_eq!(
            comparison("new name.txt", ComparisonKind::Unstaged),
            ("old name.txt\nbase\n".into(), "old name.txt\nbase\n".into())
        );

        // Reading must not record operations: that would retrigger the watcher forever.
        let watches = repository_watches(&path("kept.txt"), &trust).unwrap();
        assert_eq!(watches.paths, [root.join(".jj/working_copy/checkout")]);
        let marker = || std::fs::read(&watches.paths[0]).unwrap();
        let before = marker();
        base("kept.txt");
        assert_eq!(marker(), before);
    }

    #[test]
    fn head_name_prefers_the_title_then_bookmarks_then_parents() {
        let Some(dir) = workspace() else { return };
        let jj = |args: &[&str]| {
            assert!(Command::new("jj")
                .current_dir(dir.path())
                .args(args)
                .output()
                .is_ok_and(|output| output.status.success()));
            head_name(dir.path()).unwrap().load().to_string()
        };
        assert_eq!(jj(&["bookmark", "delete", "topic"]), "[base]");
        assert_eq!(jj(&["bookmark", "create", "here"]), "here");
        let long = "a title that is much too long for a status line";
        assert_eq!(
            jj(&["describe", "--message", long]),
            "a title that is much too long for a sta…"
        );
        assert_eq!(jj(&["new", "@", "@-"]), "[merge of 2]");
    }

    #[test]
    fn diff_entries_keep_files_and_resolve_paths_from_the_root() {
        let root = Path::new("/repo");
        let entry = |status, source_type, target_type| {
            diff_entry(
                root,
                status,
                ("old.txt", source_type),
                ("new.txt", target_type),
            )
        };
        let path = || root.join("new.txt");
        assert_eq!(
            entry("added", "", "file"),
            Some(FileChange::Untracked { path: path() })
        );
        assert_eq!(
            entry("modified", "file", "symlink"),
            Some(FileChange::Modified { path: path() })
        );
        assert_eq!(
            entry("removed", "file", ""),
            Some(FileChange::Deleted { path: path() })
        );
        assert_eq!(
            entry("modified", "file", "conflict"),
            Some(FileChange::Conflict { path: path() })
        );
        assert_eq!(
            entry("renamed", "file", "file"),
            Some(FileChange::Renamed {
                from_path: root.join("old.txt"),
                to_path: path(),
            })
        );
        assert_eq!(entry("modified", "git-submodule", "git-submodule"), None);
        assert_eq!(entry("modified", "file", "tree"), None);
    }
}

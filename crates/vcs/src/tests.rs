use super::*;

#[tokio::test]
async fn unavailable_repository_reports_one_terminal_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_owned();
    let (sender, mut responses) = tokio::sync::mpsc::unbounded_channel();
    for_each_file_status(
        ChangedFileScope::Repository(path.clone()),
        false,
        move |root, result| {
            assert_eq!(root, path);
            sender.send(result).unwrap();
            true
        },
    )
    .await
    .unwrap();
    let error = responses.recv().await.unwrap().unwrap_err();
    assert_eq!(error.to_string(), "no diff provider returns success");
    assert!(responses.recv().await.is_none());
}

#[cfg(not(feature = "git"))]
#[test]
fn disabled_git_has_no_decorations_or_watches_and_rejects_explicit_queries() {
    let path = Path::new("file");
    let decoration = inspect_file(path, false, true);
    assert!(decoration.diff_base.is_none());
    assert!(decoration.head_name.is_none());
    let watches = repository_watches(path, false);
    assert!(watches.paths.is_empty());
    assert!(watches.files.is_empty());
    assert_eq!(
        file_status(path, false).unwrap_err().to_string(),
        "No VCS provider available"
    );
    let status = FileStatus {
        staged: None,
        unstaged: Some(FileChange::Modified { path: path.into() }),
    };
    assert_eq!(
        read_comparison(&status, ComparisonKind::Head, false)
            .unwrap_err()
            .to_string(),
        "No VCS provider available"
    );
}

#[cfg(feature = "git")]
#[test]
fn inspection_preserves_git_diff_status_and_comparison_behavior() {
    use crate::git::test::{empty_git_repo, exec_git_cmd};
    let repo = empty_git_repo();
    let path = repo.path().join("file.txt");
    std::fs::write(&path, b"before").unwrap();
    exec_git_cmd(&["add", "file.txt"], repo.path());
    exec_git_cmd(&["commit", "-m", "initial"], repo.path());
    std::fs::write(&path, b"after").unwrap();
    let decoration = inspect_file(&path, false, true);
    assert_eq!(decoration.diff_base.unwrap(), b"before");
    assert_eq!(&***decoration.head_name.unwrap().load(), "main");
    let status = file_status(&path, false).unwrap().unwrap();
    let comparison = read_comparison(&status, ComparisonKind::Unstaged, false).unwrap();
    assert_eq!(comparison.before, b"before");
    assert_eq!(comparison.after, b"after");
}

#[test]
fn watch_refresh_inspects_each_directory_and_trust_pair_once() {
    let mut inspected = Vec::new();
    let paths = [
        (Path::new("repo"), false),
        (Path::new("repo"), false),
        (Path::new("repo/nested"), false),
        (Path::new("repo"), true),
        (Path::new("linked"), false),
    ];
    let watches = super::collect_repository_watches(paths, |path, trusted| {
        inspected.push((path.to_owned(), trusted));
        RepositoryWatches {
            paths: vec![path.join("index")],
            files: vec![path.join("ignored-revs")],
        }
    });
    assert_eq!(inspected.len(), 4);
    assert_eq!(watches.paths.len(), 4);
    assert_eq!(watches.files.len(), 4);
    assert!(inspected.contains(&(Path::new("repo").to_owned(), false)));
    assert!(inspected.contains(&(Path::new("repo").to_owned(), true)));
    assert!(inspected.contains(&(Path::new("repo/nested").to_owned(), false)));
    assert!(inspected.contains(&(Path::new("linked").to_owned(), false)));
    // A subsequent refresh inspects again rather than retaining configuration or trust.
    super::collect_repository_watches(paths, |path, trusted| {
        inspected.push((path.to_owned(), trusted));
        RepositoryWatches::default()
    });
    assert_eq!(inspected.len(), 8);
}

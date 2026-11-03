use super::*;

fn registry(providers: &[Option<&'static str>]) -> DiffProviderRegistry {
    DiffProviderRegistry {
        providers: providers.iter().copied().map(DiffProvider::Test).collect(),
    }
}

#[test]
fn decoration_and_review_queries_use_the_first_successful_provider() {
    let providers = registry(&[None, Some("first"), Some("second")]);
    let path = Path::new("file");
    let decoration = providers.inspect_file(path, false, true);
    assert_eq!(decoration.diff_base.unwrap(), b"first");
    assert_eq!(&***decoration.head_name.unwrap().load(), "first");
    let binary = providers.inspect_file(path, false, false);
    assert!(binary.diff_base.is_none());
    assert_eq!(&***binary.head_name.unwrap().load(), "first");
    let status = providers.file_status(path, false).unwrap().unwrap();
    let comparison = providers
        .read_comparison(&status, ComparisonKind::Head, false)
        .unwrap();
    assert_eq!(comparison.before, b"first");
}

#[test]
fn watch_inputs_include_every_available_provider() {
    let providers = registry(&[None, Some("first"), Some("second")]);
    let expected = [PathBuf::from("first"), PathBuf::from("second")];
    let watches = providers.repository_watches(Path::new("repo"), false);
    assert_eq!(watches.paths, expected);
    assert_eq!(watches.files, expected);
}

async fn statuses(providers: DiffProviderRegistry) -> Vec<Result<FileStatus>> {
    let (sender, mut responses) = tokio::sync::mpsc::unbounded_channel();
    providers.for_each_file_status(
        ChangedFileScope::Repository("repo".into()),
        false,
        move |root, result| {
            assert_eq!(root, Path::new("repo"));
            sender.send(result).unwrap();
            true
        },
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut results = Vec::new();
        while let Some(result) = responses.recv().await {
            results.push(result);
        }
        results
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn enumeration_uses_one_provider_and_stops_on_successful_empty_results() {
    let results = statuses(registry(&[None, Some("first"), Some("second")])).await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].as_ref().unwrap().path(), Path::new("repo/first"));
    assert!(statuses(registry(&[Some(""), Some("second")]))
        .await
        .is_empty());
    assert!(registry(&[Some(""), Some("second")])
        .file_status(Path::new("file"), false)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn empty_or_unavailable_registries_report_one_terminal_error() {
    for providers in [registry(&[]), registry(&[None, None])] {
        let results = statuses(providers).await;
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].as_ref().unwrap_err().to_string(),
            "no diff provider returns success"
        );
    }
    #[cfg(not(feature = "git"))]
    assert!(DiffProviderRegistry::default().providers.is_empty());
}

#[cfg(feature = "git")]
#[test]
fn fallback_keeps_git_diff_status_and_comparison_behavior() {
    use crate::git::test::{empty_git_repo, exec_git_cmd};
    let repo = empty_git_repo();
    let path = repo.path().join("file.txt");
    std::fs::write(&path, b"before").unwrap();
    exec_git_cmd(&["add", "file.txt"], repo.path());
    exec_git_cmd(&["commit", "-m", "initial"], repo.path());
    std::fs::write(&path, b"after").unwrap();
    let providers = DiffProviderRegistry {
        providers: vec![DiffProvider::Test(None), DiffProvider::Git].into(),
    };
    let decoration = providers.inspect_file(&path, false, true);
    assert_eq!(decoration.diff_base.unwrap(), b"before");
    assert_eq!(&***decoration.head_name.unwrap().load(), "main");
    let status = providers.file_status(&path, false).unwrap().unwrap();
    let comparison = providers
        .read_comparison(&status, ComparisonKind::Unstaged, false)
        .unwrap();
    assert_eq!(comparison.before, b"before");
    assert_eq!(comparison.after, b"after");
}

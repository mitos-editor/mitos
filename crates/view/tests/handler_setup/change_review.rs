use std::sync::Arc;

use editor_core::Selection;
use vcs::{FileChange, FileStatus};
use view::{
    current, current_ref,
    editor::Action,
    features::change_review::{HunkReviewRequest, ReviewComparison, ReviewRequest},
};

#[cfg(feature = "syntax-diff")]
const JSON_LANGUAGE: &str = r#"
[[language]]
name = "review-json"
scope = "source.json"
file-types = ["words"]
grammar = "json"
"#;

#[cfg(feature = "syntax-diff")]
#[tokio::test]
async fn review_engine_policy_cache_and_captured_language_resources_follow_configuration(
) -> anyhow::Result<()> {
    use editor_config::DiffEngine;
    let mut f = Fixture::with_languages("{\"value\": 1}\n", JSON_LANGUAGE)?;
    prepare(&mut f, "initial");
    f.replace("{\n  \"value\": 2\n}\n");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let status = FileStatus {
        staged: None,
        unstaged: Some(FileChange::Modified { path }),
    };
    let line = ReviewRequest::new(&f.editor, &status, ReviewComparison::Buffer, f.dir.path());
    let original = line.compute_cached(None)?;
    assert_eq!(original.review.provider, None);
    assert!(original.review.fallback.is_none());
    let hunk = HunkReviewRequest::capture(&f.editor)?;
    f.configure(|config| config.diff.engine = DiffEngine::Syntax);
    assert!(!line.is_current(&f.editor));
    assert!(!hunk.is_current(&f.editor));
    let syntax = ReviewRequest::new(&f.editor, &status, ReviewComparison::Buffer, f.dir.path());
    assert!(!line.same_target(&syntax));
    let updated = syntax.compute_cached(Some(original.clone()))?;
    assert!(!Arc::ptr_eq(&updated, &original));
    assert_eq!(updated.review.provider, Some("syntax"));
    assert_eq!(
        updated
            .source
            .compute(Some(DiffEngine::Line), &|| false)
            .provider,
        None
    );
    // A source is immutable even if the live document changes after opening the review.
    f.replace("{\"value\": 3}\n");
    let captured = updated.source.compute(Some(DiffEngine::Syntax), &|| false);
    assert!(captured.lines.iter().any(|line| line.text.contains('2')));
    assert!(!captured
        .lines
        .iter()
        .any(|line| line.text.contains("\"value\": 3")));
    let syntax = ReviewRequest::new(&f.editor, &status, ReviewComparison::Buffer, f.dir.path());
    let overridden = syntax.clone().with_engine(Some(DiffEngine::Line));
    assert!(!syntax.same_target(&overridden));
    assert!(overridden.is_current(&f.editor));
    f.editor.syn_loader.store(Arc::new(language::Loader::new(
        toml::from_str(JSON_LANGUAGE)?,
        loader::syntax::Resources::default(),
    )?));
    assert!(!syntax.is_current(&f.editor));
    Ok(())
}

#[cfg(feature = "syntax-diff")]
#[tokio::test]
async fn language_policy_overrides_global_defaults_for_hunk_and_unopened_file_reviews(
) -> anyhow::Result<()> {
    use editor_config::DiffEngine;
    let languages = format!("{JSON_LANGUAGE}\ndiff-engine = 'syntax'\n");
    let mut f = Fixture::with_languages("{\"value\": 1}\n", &languages)?;
    prepare(&mut f, "initial");
    f.replace("{\"value\": 2}\n");
    let hunk = HunkReviewRequest::capture(&f.editor)?.compute()?;
    assert_eq!(hunk.provider, Some("syntax"));
    let path = f.dir.path().join("unopened.words");
    std::fs::write(&path, "{\"added\": 3}\n")?;
    let status = FileStatus {
        staged: None,
        unstaged: Some(FileChange::Untracked { path }),
    };
    let request = ReviewRequest::new(&f.editor, &status, ReviewComparison::Unstaged, f.dir.path());
    assert_eq!(request.engine(), DiffEngine::Syntax);
    assert_eq!(request.compute()?.provider, Some("syntax"));
    let index = std::fs::read(f.dir.path().join(".git/index"))?;
    let request = request.with_engine(Some(DiffEngine::Line));
    assert_eq!(request.compute()?.provider, None);
    assert_eq!(std::fs::read(f.dir.path().join(".git/index"))?, index);
    Ok(())
}

#[tokio::test]
async fn explicit_syntax_failures_keep_textual_review_and_auto_can_choose_line(
) -> anyhow::Result<()> {
    use editor_config::DiffEngine;
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    f.replace("unsaved\n");
    f.configure(|config| config.diff.engine = DiffEngine::Syntax);
    let review = HunkReviewRequest::capture(&f.editor)?.compute()?;
    assert_eq!(review.provider, None);
    assert!(review.fallback.is_some());
    assert!(review.lines.iter().any(|line| line.text == "unsaved"));
    #[cfg(feature = "syntax-diff")]
    {
        f.configure(|config| config.diff.engine = DiffEngine::Auto);
        let review = HunkReviewRequest::capture(&f.editor)?.compute()?;
        assert_eq!(review.provider, None);
        assert_eq!(review.fallback, None);
    }
    Ok(())
}

use super::{
    blame::{git, prepare},
    Fixture,
};

#[tokio::test]
async fn hunk_reviews_reject_changed_selection_text_focus_and_newer_requests() -> anyhow::Result<()>
{
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    f.replace("unsaved\n");
    let request = HunkReviewRequest::capture(&f.editor)?;
    let review = request.compute()?;
    assert_eq!((review.additions, review.deletions), (1, 1));
    assert!(request.is_current(&f.editor));
    let (view, doc) = current!(f.editor);
    doc.set_selection(view.id, Selection::point(2));
    assert!(!request.is_current(&f.editor));
    let request = HunkReviewRequest::capture(&f.editor)?;
    let newer = HunkReviewRequest::capture(&f.editor)?;
    assert!(!request.is_current(&f.editor));
    assert!(newer.is_current(&f.editor));
    f.replace("another edit\n");
    assert!(!newer.is_current(&f.editor));
    let request = HunkReviewRequest::capture(&f.editor)?;
    f.editor.new_file(Action::VerticalSplit);
    assert!(!request.is_current(&f.editor));
    Ok(())
}

#[tokio::test]
async fn repository_review_cache_reuses_unchanged_snapshots_and_follows_staging(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    std::fs::write(&path, "disk edit\n")?;
    let status = FileStatus {
        staged: Some(FileChange::Modified { path: path.clone() }),
        unstaged: Some(FileChange::Modified { path }),
    };
    let request = ReviewRequest::new(&f.editor, &status, ReviewComparison::Staged, f.dir.path());
    let original = request.compute_cached(None)?;
    let cached = request.compute_cached(Some(original.clone()))?;
    assert!(Arc::ptr_eq(&original, &cached));
    assert_eq!(original.review.additions, 0);
    let unstaged = ReviewRequest::new(&f.editor, &status, ReviewComparison::Unstaged, f.dir.path());
    let unstaged = unstaged.compute_cached(Some(original.clone()))?;
    assert_eq!(unstaged.review.additions, 1);
    assert!(unstaged
        .review
        .lines
        .iter()
        .any(|line| line.text == "disk edit"));
    git(f.dir.path(), &["add", "-A"]);
    let updated = request.compute_cached(Some(original.clone()))?;
    assert!(!Arc::ptr_eq(&original, &updated));
    assert!(updated
        .review
        .lines
        .iter()
        .any(|line| line.text == "disk edit"));
    assert!(updated
        .review
        .lines
        .iter()
        .any(|line| line.text == "committed"));
    Ok(())
}

#[tokio::test]
async fn reviews_honor_explicit_encoding_and_reject_results_after_encoding_changes(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("café\n")?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let bytes: Vec<_> = "café\n".encode_utf16().flat_map(u16::to_le_bytes).collect();
    std::fs::write(&path, &bytes)?;
    git(f.dir.path(), &["add", "-A"]);
    git(f.dir.path(), &["commit", "-qm", "encoded"]);
    current!(f.editor).1.set_encoding("utf-16le")?;
    f.replace("caféteria\n");
    let status = FileStatus {
        staged: None,
        unstaged: Some(FileChange::Modified { path: path.clone() }),
    };
    let buffer = ReviewRequest::new(&f.editor, &status, ReviewComparison::Buffer, f.dir.path());
    let original = buffer.compute_cached(None)?;
    let hunk = HunkReviewRequest::capture(&f.editor)?;
    for review in [original.review.as_ref(), &hunk.compute()?] {
        assert_eq!((review.additions, review.deletions), (1, 1));
        assert!(review.lines.iter().any(|line| line.text == "café"));
        assert!(review.lines.iter().any(|line| line.text == "caféteria"));
    }
    let disk = ReviewRequest::new(&f.editor, &status, ReviewComparison::Unstaged, f.dir.path());
    assert_eq!(disk.compute()?.additions, 0);
    // Encoding changes do not increment the text version.
    current!(f.editor).1.set_encoding("utf-8")?;
    assert!(!buffer.is_current(&f.editor));
    assert!(!disk.is_current(&f.editor));
    assert!(!hunk.is_current(&f.editor));
    let updated = ReviewRequest::new(&f.editor, &status, ReviewComparison::Buffer, f.dir.path());
    assert!(!updated.same_target(&buffer));
    assert!(updated.compute_cached(Some(original)).is_err());
    assert_eq!(std::fs::read(path)?, bytes);
    Ok(())
}

#[tokio::test]
async fn repository_attributes_invalidate_cached_worktree_conversions() -> anyhow::Result<()> {
    let mut f = Fixture::new("committed\nsecond\n")?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    std::fs::write(&path, "disk edit\nsecond\n")?;
    let status = FileStatus {
        staged: None,
        unstaged: Some(FileChange::Modified { path }),
    };
    let request = ReviewRequest::new(&f.editor, &status, ReviewComparison::Unstaged, f.dir.path());
    let original = request.compute_cached(None)?;
    assert_eq!(
        (original.review.additions, original.review.deletions),
        (1, 1)
    );
    let attributes = f.dir.path().join(".git/info/attributes");
    std::fs::write(&attributes, "* text eol=crlf\n")?;
    assert!(original.affected_by(&attributes.canonicalize()?));
    let updated = request.compute_cached(Some(original.clone()))?;
    assert!(!Arc::ptr_eq(&original, &updated));
    assert_eq!((updated.review.additions, updated.review.deletions), (2, 2));
    Ok(())
}

#[tokio::test]
async fn buffer_review_uses_the_index_without_saving_and_invalidates_after_edits(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    std::fs::write(&path, "staged\n")?;
    git(f.dir.path(), &["add", "-A"]);
    f.replace("unsaved\n");
    let index = std::fs::read(f.dir.path().join(".git/index"))?;
    let status = FileStatus {
        staged: Some(FileChange::Modified { path: path.clone() }),
        unstaged: None,
    };
    let request = ReviewRequest::new(&f.editor, &status, ReviewComparison::Buffer, f.dir.path());
    let review = request.compute()?;
    assert!(review.lines.iter().any(|line| line.text == "staged"));
    assert!(review.lines.iter().any(|line| line.text == "unsaved"));
    assert!(!review.lines.iter().any(|line| line.text == "committed"));
    assert!(request.is_current(&f.editor));
    f.replace("newer edit\n");
    assert!(!request.is_current(&f.editor));
    assert_eq!(std::fs::read_to_string(path)?, "staged\n");
    assert_eq!(std::fs::read(f.dir.path().join(".git/index"))?, index);
    Ok(())
}

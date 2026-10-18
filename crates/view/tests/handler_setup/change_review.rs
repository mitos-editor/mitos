use std::sync::Arc;

use editor_core::Selection;
use vcs::{FileChange, FileStatus};
use view::{
    current, current_ref,
    editor::Action,
    features::change_review::{HunkReviewRequest, ReviewComparison, ReviewRequest},
};

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

use std::sync::Arc;
use stdx::rope::RopeExt as _;

use editor_core::Selection;
use vcs::{FileChange, FileStatus};
use view::{
    current, current_ref,
    editor::Action,
    features::change_review::{HunkReviewRequest, ReviewComparison, ReviewRequest},
};

pub(super) async fn finish_review(f: &mut Fixture, id: view::DocumentId) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while f
            .editor
            .document(id)
            .unwrap()
            .review()
            .unwrap()
            .is_loading()
        {
            let callback = f
                .callbacks
                .recv()
                .await
                .expect("review callback channel closed");
            callback(&mut f.editor);
        }
    })
    .await?;
    Ok(())
}

pub(super) fn review_request(
    f: &Fixture,
    path: &std::path::Path,
    comparison: ReviewComparison,
) -> ReviewRequest {
    ReviewRequest::new(
        &f.editor,
        &FileStatus {
            staged: None,
            unstaged: Some(FileChange::Modified { path: path.into() }),
        },
        comparison,
        f.dir.path(),
    )
    .with_fresh_status()
}

#[tokio::test(flavor = "multi_thread")]
async fn review_buffers_are_native_documents_with_independent_splits_and_no_file_identity(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("committed\nkeep\n")?;
    prepare(&mut f, "initial");
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    f.replace("unsaved\nkeep\n");
    let first_view = f.editor.tree.focus();
    let first = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    finish_review(&mut f, first).await?;
    let doc = f.editor.document(first).unwrap();
    assert!(doc.path().is_none());
    assert!(doc.uri().is_none());
    assert!(doc.is_readonly() && !doc.is_modified());
    assert!(doc.display_name().contains("[diff"));
    assert_eq!(doc.cursor_line(first_view), 0);
    assert_eq!(doc.view_offset(first_view).anchor, 0);
    assert!(doc.text().to_string().contains("unsaved"));
    let first_text = doc.text().clone();

    let second = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Staged),
        Action::VerticalSplit,
        None,
    );
    finish_review(&mut f, second).await?;
    assert_ne!(first, second);
    assert_eq!(f.editor.tree.views().count(), 2);
    assert_eq!(f.editor.tree.get(first_view).document_id(), first);
    assert_eq!(f.editor.document(first).unwrap().text(), &first_text);
    assert_eq!(
        f.editor.document(source).unwrap().text().to_string(),
        "unsaved\nkeep\n"
    );
    assert_eq!(std::fs::read_to_string(&path)?, "committed\nkeep\n");

    f.editor.switch(first, Action::Replace);
    let second_view = f.editor.tree.focus();
    let doc = f.editor.document_mut(first).unwrap();
    doc.set_selection(first_view, Selection::point(doc.text().line_to_char_idx(1)));
    doc.set_selection(
        second_view,
        Selection::point(doc.text().line_to_char_idx(2)),
    );
    assert_ne!(doc.cursor_line(first_view), doc.cursor_line(second_view));
    assert!(
        f.editor.document(second).is_some(),
        "switching away must retain a review buffer"
    );
    assert!(f.editor.close_document(second, false).is_ok());
    assert!(f.editor.document(first).is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn review_protection_covers_editing_history_modes_and_forced_writes() -> anyhow::Result<()> {
    use editor_core::Transaction;
    use view::editing::EditError;
    let mut f = Fixture::new("before\n")?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    f.replace("after\n");
    let id = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    finish_review(&mut f, id).await?;
    let version = f.editor.document(id).unwrap().version();
    let text = f.editor.document(id).unwrap().text().clone();
    let exported = f.dir.path().join("must-not-exist.txt");
    let trust = f.editor.diff_providers.clone();
    let (view, doc) = current!(f.editor);
    let edit = Transaction::change(
        doc.text(),
        std::iter::once((0, 0, Some("forbidden".into()))),
    );
    assert_eq!(doc.apply(edit.changes()), Err(EditError::ReadOnlyDocument));
    assert_eq!(
        view::editing::apply(doc, &edit, view.id),
        Err(EditError::ReadOnlyDocument)
    );
    assert!(!doc.undo() && !doc.redo());
    assert!(doc.save(Some(exported.clone()), true).is_err());
    assert!(doc.reload(&trust, true).is_err());
    assert_eq!(doc.version(), version);
    assert_eq!(doc.text(), &text);
    f.editor.set_mode(view::document::Mode::Insert);
    assert_eq!(f.editor.mode(), view::document::Mode::Normal);
    assert!(f.editor.save(id, Some(exported.clone()), true).is_err());
    f.editor.set_doc_path(id, &exported);
    assert!(f.editor.document(id).unwrap().path().is_none());
    assert!(!exported.exists());
    assert!(!f.editor.document(id).unwrap().is_modified());
    assert!(f.editor.close_document(id, false).is_ok());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_maps_each_splits_selection_and_does_not_steal_focus() -> anyhow::Result<()> {
    use editor_core::Transaction;
    let before: String = (0..35).map(|i| format!("line {i}\n")).collect();
    let mut f = Fixture::new(&before)?;
    prepare(&mut f, "initial");
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    f.replace(
        &before
            .replace("line 2\n", "changed\n")
            .replace("line 25\n", "other\n"),
    );
    let id = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    finish_review(&mut f, id).await?;
    let first = f.editor.tree.focus();
    f.editor.switch(id, Action::VerticalSplit);
    let second = f.editor.tree.focus();
    let doc = f.editor.document_mut(id).unwrap();
    let row = |needle: &str| {
        doc.text()
            .lines(stdx::rope::LINE_TYPE)
            .position(|line| line.to_string().trim_end() == needle)
            .unwrap()
    };
    let first_pos = doc.text().line_to_char_idx(row("line 4"));
    let second_pos = doc.text().line_to_char_idx(row("line 24"));
    doc.set_selection(first, Selection::point(first_pos));
    doc.set_selection(second, Selection::point(second_pos));
    let source_doc = f.editor.document(source).unwrap();
    let edit = Transaction::change(
        source_doc.text(),
        std::iter::once((0, 0, Some("inserted\n".into()))),
    );
    f.editor
        .apply_document_changes(source, source_doc.version(), edit.changes())?;
    f.editor.new_file(Action::Replace);
    let focused = current_ref!(f.editor).1.id();
    wait_for_review(&mut f, id, |doc| {
        doc.text().to_string().contains("inserted")
    })
    .await?;
    assert_eq!(current_ref!(f.editor).1.id(), focused);
    let doc = f.editor.document(id).unwrap();
    for (view, expected) in [(first, "line 4"), (second, "line 24")] {
        assert_eq!(
            doc.text()
                .line(doc.cursor_line(view), stdx::rope::LINE_TYPE)
                .to_string()
                .trim_end(),
            expected
        );
    }
    assert!(!doc.is_modified());
    f.editor.jump_backward(second, 1);
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.id(), id);
    assert_eq!(
        doc.text()
            .line(doc.cursor_line(view.id), stdx::rope::LINE_TYPE)
            .to_string()
            .trim_end(),
        "line 24"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn canceled_or_closed_review_callbacks_cannot_replace_a_newer_document() -> anyhow::Result<()>
{
    let mut f = Fixture::with_config("before\n", "language = []", Default::default(), |config| {
        config.completion.word.enable = false
    })?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    f.replace("after\n");
    while let Ok(callback) = f.callbacks.try_recv() {
        callback(&mut f.editor);
    }
    let id = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    let old = tokio::time::timeout(std::time::Duration::from_secs(10), f.callbacks.recv())
        .await?
        .unwrap();
    f.editor.refresh_review()?;
    old(&mut f.editor);
    assert!(f
        .editor
        .document(id)
        .unwrap()
        .review()
        .unwrap()
        .is_loading());
    finish_review(&mut f, id).await?;
    f.editor.refresh_review()?;
    let closed = tokio::time::timeout(std::time::Duration::from_secs(10), f.callbacks.recv())
        .await?
        .unwrap();
    assert!(f.editor.close_document(id, false).is_ok());
    let next = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Staged),
        Action::Replace,
        None,
    );
    closed(&mut f.editor);
    finish_review(&mut f, next).await?;
    assert_ne!(id, next);
    assert_eq!(
        f.editor
            .document(next)
            .unwrap()
            .review()
            .unwrap()
            .comparison(),
        ReviewComparison::Staged
    );
    assert!(f.editor.document(id).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn review_gutters_show_sources_and_navigation_validates_live_text() -> anyhow::Result<()> {
    let mut f = Fixture::new("removed\nkeep\n")?;
    prepare(&mut f, "initial");
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    f.replace("keep\n");
    let id = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    finish_review(&mut f, id).await?;
    let (view, doc) = current_ref!(f.editor);
    let review = doc.review().unwrap();
    let removed = review
        .review()
        .lines
        .iter()
        .position(|row| row.kind == view::features::change_review::ReviewLineKind::Removed)
        .unwrap();
    let row = doc.review().unwrap().row(removed).unwrap();
    assert_eq!(row.before, Some(0));
    assert_eq!(row.after, None);
    assert_eq!(
        view::gutter::width(editor_config::GutterType::LineNumbers, view, doc),
        3
    );
    let view_id = view.id;
    let doc = f.editor.document_mut(id).unwrap();
    doc.set_selection(
        view_id,
        Selection::point(doc.text().line_to_char_idx(removed)),
    );
    let source_doc = f.editor.document(source).unwrap();
    let edit = editor_core::Transaction::change(
        source_doc.text(),
        std::iter::once((0, 0, Some("newer\n".into()))),
    );
    f.editor
        .apply_document_changes(source, source_doc.version(), edit.changes())?;
    assert!(f.editor.open_review_source(true, Action::Replace).is_err());
    assert_eq!(current_ref!(f.editor).1.id(), id);
    f.editor.open_review_source(false, Action::Replace)?;
    assert_eq!(current_ref!(f.editor).1.id(), source);
    assert!(f.editor.document(id).is_some());
    Ok(())
}

#[cfg(feature = "syntax-diff")]
#[tokio::test(flavor = "multi_thread")]
async fn review_refresh_uses_configured_engine_and_cancels_older_work() -> anyhow::Result<()> {
    use editor_config::DiffEngine;
    let mut f = Fixture::with_languages("{\"λ\": 1}\n", JSON_LANGUAGE)?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    f.replace("{\"λ\": 2}\n");
    let id = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    f.configure(|config| config.diff.engine = DiffEngine::Syntax);
    f.editor.refresh_review()?;
    f.configure(|config| config.diff.engine = DiffEngine::Line);
    f.editor.refresh_review()?;
    finish_review(&mut f, id).await?;
    let doc = f.editor.document(id).unwrap();
    let text = doc.text().clone();
    let source = doc.review().unwrap().source().unwrap().clone();
    assert_eq!(doc.review().unwrap().engine(), DiffEngine::Line);
    assert!(!doc.word_completion_enabled());
    f.configure(|config| config.diff.engine = DiffEngine::Syntax);
    f.editor.refresh_review()?;
    finish_review(&mut f, id).await?;
    let doc = f.editor.document(id).unwrap();
    assert_eq!(doc.review().unwrap().review().provider, Some("syntax"));
    assert_eq!(doc.text(), &text);
    assert_eq!(doc.review().unwrap().engine(), DiffEngine::Syntax);
    assert_eq!(
        doc.review().unwrap().source().unwrap().after(),
        source.after()
    );
    assert!(!doc.is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn full_file_head_review_captures_unsaved_text_and_resolves_staged_renames(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("committed\nsecond\nthird\nfourth\n")?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    std::fs::write(&path, "staged\nsecond\nthird\nfourth\n")?;
    git(f.dir.path(), &["add", "-A"]);
    f.replace("unsaved\nsecond\nthird\nfourth\n");
    let index = std::fs::read(f.dir.path().join(".git/index"))?;
    let status = FileStatus {
        staged: None,
        unstaged: Some(FileChange::Modified { path: path.clone() }),
    };
    let request = ReviewRequest::new(&f.editor, &status, ReviewComparison::Head, f.dir.path())
        .with_fresh_status();
    let mut task = event::TaskController::new();
    let snapshot = Arc::new(request.clone()).run(None, task.restart()).await?;
    assert_eq!(
        snapshot.source.after().to_string(),
        "unsaved\nsecond\nthird\nfourth\n"
    );
    assert!(snapshot
        .diff()?
        .lines
        .iter()
        .any(|line| line.text == "committed"));
    assert!(!snapshot
        .diff()?
        .lines
        .iter()
        .any(|line| line.text == "staged"));
    f.replace("newer\n");
    assert!(!request.is_current(&f.editor));
    assert_eq!(
        std::fs::read_to_string(&path)?,
        "staged\nsecond\nthird\nfourth\n"
    );
    assert_eq!(std::fs::read(f.dir.path().join(".git/index"))?, index);

    let renamed = path.with_file_name("renamed.txt");
    git(
        f.dir.path(),
        &[
            "mv",
            path.file_name().unwrap().to_str().unwrap(),
            "renamed.txt",
        ],
    );
    let status = FileStatus {
        staged: None,
        unstaged: Some(FileChange::Modified { path: renamed }),
    };
    let review = ReviewRequest::new(&f.editor, &status, ReviewComparison::Staged, f.dir.path())
        .with_fresh_status()
        .compute()?;
    assert!(review.lines.iter().any(|line| line.kind
        == view::features::change_review::ReviewLineKind::Removed
        && line.text == "committed"));
    assert!(review.lines.iter().any(|line| line.kind
        == view::features::change_review::ReviewLineKind::Added
        && line.text == "staged"));
    Ok(())
}

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
    assert_eq!(original.diff()?.provider, None);
    assert!(original.diff()?.fallback.is_none());
    let hunk = HunkReviewRequest::capture(&f.editor)?;
    f.configure(|config| config.diff.engine = DiffEngine::Syntax);
    assert!(!line.is_current(&f.editor));
    assert!(!hunk.is_current(&f.editor));
    let syntax = ReviewRequest::new(&f.editor, &status, ReviewComparison::Buffer, f.dir.path());
    assert!(!line.same_target(&syntax));
    let updated = syntax.compute_cached(Some(original.clone()))?;
    assert!(!Arc::ptr_eq(&updated, &original));
    assert_eq!(updated.diff()?.provider, Some("syntax"));
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
    let original = syntax.compute_cached(None)?;
    let cached = syntax.compute_cached(Some(original.clone()))?;
    assert!(Arc::ptr_eq(&original, &cached));
    f.configure(|config| config.diff.engine = DiffEngine::Line);
    let line = ReviewRequest::new(&f.editor, &status, ReviewComparison::Buffer, f.dir.path());
    let updated = line.compute_cached(Some(original.clone()))?;
    assert_eq!(updated.engine(), DiffEngine::Line);
    assert_eq!(updated.diff()?.provider, None);
    assert!(!Arc::ptr_eq(&original.source, &updated.source));
    f.editor.syn_loader.store(Arc::new(language::Loader::new(
        toml::from_str(JSON_LANGUAGE)?,
        loader::syntax::Resources::default(),
    )?));
    assert!(!syntax.is_current(&f.editor));
    Ok(())
}

#[cfg(feature = "syntax-diff")]
#[tokio::test]
async fn canceled_syntax_comparisons_do_not_poison_cached_presentations() -> anyhow::Result<()> {
    use editor_config::DiffEngine;
    let mut f = Fixture::with_languages("{\"value\": 1}\n", JSON_LANGUAGE)?;
    prepare(&mut f, "initial");
    f.replace("{\"value\": 2}\n");
    let (source, line) = HunkReviewRequest::capture(&f.editor)?.compute_review()?;
    assert!(Arc::ptr_eq(
        &line,
        &source.compute(Some(DiffEngine::Line), &|| false)
    ));
    // Cancel after exact rows are ready, during the provider comparison.
    let checks = std::cell::Cell::new(0);
    source.compute(Some(DiffEngine::Syntax), &|| {
        checks.set(checks.get() + 1);
        checks.get() >= 3
    });
    let syntax = source.compute(Some(DiffEngine::Syntax), &|| false);
    assert_eq!(syntax.provider, Some("syntax"));
    assert!(syntax.fallback.is_none());
    assert!(Arc::ptr_eq(
        &syntax,
        &source.compute(Some(DiffEngine::Auto), &|| false)
    ));
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
    f.configure(|config| config.diff.engine = DiffEngine::Line);
    let request = ReviewRequest::new(&f.editor, &status, ReviewComparison::Unstaged, f.dir.path());
    assert_eq!(request.compute()?.provider, Some("syntax"));
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
    assert_eq!(original.diff()?.additions, 0);
    let unstaged = ReviewRequest::new(&f.editor, &status, ReviewComparison::Unstaged, f.dir.path());
    let unstaged = unstaged.compute_cached(Some(original.clone()))?;
    assert_eq!(unstaged.diff()?.additions, 1);
    assert!(unstaged
        .diff()?
        .lines
        .iter()
        .any(|line| line.text == "disk edit"));
    git(f.dir.path(), &["add", "-A"]);
    let updated = request.compute_cached(Some(original.clone()))?;
    assert!(!Arc::ptr_eq(&original, &updated));
    assert!(updated
        .diff()?
        .lines
        .iter()
        .any(|line| line.text == "disk edit"));
    assert!(updated
        .diff()?
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
    let inspected = hunk.compute()?;
    for review in [original.diff()?.as_ref(), inspected.as_ref()] {
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
        (original.diff()?.additions, original.diff()?.deletions),
        (1, 1)
    );
    let attributes = f.dir.path().join(".git/info/attributes");
    std::fs::write(&attributes, "* text eol=crlf\n")?;
    assert!(original.affected_by(&attributes.canonicalize()?));
    let updated = request.compute_cached(Some(original.clone()))?;
    assert!(!Arc::ptr_eq(&original, &updated));
    assert_eq!(
        (updated.diff()?.additions, updated.diff()?.deletions),
        (2, 2)
    );
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

#[tokio::test(flavor = "multi_thread")]
async fn review_labels_are_stable_disambiguate_sources_and_number_independent_snapshots(
) -> anyhow::Result<()> {
    use view::document::DocumentKind;
    let mut f = Fixture::new("before\n")?;
    prepare(&mut f, "initial");
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    f.replace("after\n");
    let first = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    let name = f
        .editor
        .document(first)
        .unwrap()
        .display_name()
        .into_owned();
    assert!(name.ends_with("document.words [diff: HEAD]"));
    finish_review(&mut f, first).await?;
    f.configure(|config| config.diff.engine = editor_config::DiffEngine::Syntax);
    f.editor.refresh_review()?;
    finish_review(&mut f, first).await?;
    assert_eq!(f.editor.document(first).unwrap().display_name(), name);
    assert_eq!(
        f.editor.document(first).unwrap().display_metadata().kind,
        DocumentKind::Diff {
            comparison: ReviewComparison::Head,
            instance: 1
        }
    );
    assert_eq!(
        f.editor
            .document(first)
            .unwrap()
            .display_metadata()
            .source_path,
        Some(path.as_path())
    );
    assert!(f.editor.document(first).unwrap().path().is_none());
    assert!(f.editor.document(first).unwrap().uri().is_none());
    let second = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    finish_review(&mut f, second).await?;
    let second_name = f
        .editor
        .document(second)
        .unwrap()
        .display_name()
        .into_owned();
    assert!(second_name.ends_with("[diff: HEAD #2]"));
    assert!(f.editor.close_document(first, false).is_ok());
    assert_eq!(
        f.editor.document(second).unwrap().display_name(),
        second_name
    );
    let third = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    finish_review(&mut f, third).await?;
    assert!(f
        .editor
        .document(third)
        .unwrap()
        .display_name()
        .ends_with("[diff: HEAD #3]"));
    let staged = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Staged),
        Action::Replace,
        None,
    );
    finish_review(&mut f, staged).await?;
    assert!(f
        .editor
        .document(staged)
        .unwrap()
        .display_name()
        .ends_with("[diff: staged]"));
    let labels = f.editor.document_tab_labels();
    assert_eq!(labels[&source], "document.words");
    assert_eq!(labels[&second], "document.words [diff: HEAD #2]");
    assert_eq!(labels[&third], "document.words [diff: HEAD #3]");
    // Same filenames in different directories and workspaces remain distinct.
    let nested = path
        .parent()
        .unwrap()
        .join("nested")
        .join(path.file_name().unwrap());
    std::fs::create_dir_all(nested.parent().unwrap())?;
    std::fs::write(&nested, "other\n")?;
    let other = f.editor.open(&nested, Action::Load)?;
    let review = f.editor.open_review(
        review_request(&f, &nested, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    finish_review(&mut f, review).await?;
    let labels = f.editor.document_tab_labels();
    assert_ne!(labels[&source], labels[&other]);
    assert!(labels[&review].contains("nested"));
    assert!(labels[&review].ends_with("[diff: HEAD]"));
    let elsewhere = tempfile::tempdir()?;
    let alternate = elsewhere.path().join(path.file_name().unwrap());
    std::fs::write(&alternate, "elsewhere\n")?;
    let elsewhere = f.editor.open(&alternate, Action::Load)?;
    let labels = f.editor.document_tab_labels();
    assert_ne!(labels[&source], labels[&elsewhere]);
    assert_ne!(labels[&other], labels[&elsewhere]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn review_motions_preserve_cursors_selection_mode_wrapping_and_jumps() -> anyhow::Result<()> {
    use editor_core::{movement::Direction, Range};
    let original = (0..40).map(|i| format!("line {i}\n")).collect::<String>();
    let mut f = Fixture::new(&original)?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    f.replace(
        &original
            .replace("line 5\n", "changed 5\n")
            .replace("line 30\n", "changed 30\n"),
    );
    let id = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::Replace,
        None,
    );
    finish_review(&mut f, id).await?;
    let (view, doc) = current!(f.editor);
    let rows: Vec<_> = doc
        .review()
        .unwrap()
        .review()
        .lines
        .iter()
        .enumerate()
        .filter_map(|(i, row)| {
            (row.kind == view::features::change_review::ReviewLineKind::Header).then_some(i)
        })
        .collect();
    assert_eq!(rows.len(), 2);
    let first = doc.text().line_to_char_idx(rows[0]);
    let last = doc.text().line_to_char_idx(rows[1]);
    doc.set_selection(
        view.id,
        Selection::new(vec![Range::point(first), Range::point(last)].into(), 0),
    );
    f.configure(|config| config.search.wrap_around = false);
    f.editor.navigate_review_hunk(Direction::Forward, 1, false);
    assert_eq!(
        current_ref!(f.editor)
            .1
            .selection(current_ref!(f.editor).0.id)
            .primary()
            .cursor(current_ref!(f.editor).1.text().slice(..)),
        last
    );
    f.configure(|config| config.search.wrap_around = true);
    f.editor.navigate_review_hunk(Direction::Forward, 1, false);
    assert_eq!(
        current_ref!(f.editor)
            .1
            .selection(current_ref!(f.editor).0.id)
            .primary()
            .cursor(current_ref!(f.editor).1.text().slice(..)),
        first
    );
    let (view, doc) = current!(f.editor);
    doc.set_selection(
        view.id,
        Selection::new(
            vec![
                Range::point(doc.text().line_to_char_idx(rows[0] + 1)),
                Range::point(doc.text().line_to_char_idx(rows[1] + 1)),
            ]
            .into(),
            0,
        ),
    );
    f.editor.navigate_review_hunk(Direction::Backward, 1, false);
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.selection(view.id).len(), 2);
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        first
    );
    f.editor.set_mode(view::document::Mode::Select);
    f.editor.navigate_review_hunk(Direction::Forward, 5, false);
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.selection(view.id).primary().anchor, first);
    assert!(doc.selection(view.id).primary().head >= last);
    let view_id = view.id;
    f.editor.set_mode(view::document::Mode::Normal);
    let doc = f.editor.document_mut(id).unwrap();
    let origin = doc.text().line_to_char_idx(rows[0] + 1);
    doc.set_selection(view_id, Selection::point(origin));
    f.editor.navigate_review_hunk(Direction::Forward, 1, false);
    f.editor.jump_backward(view_id, 1);
    let doc = f.editor.document(id).unwrap();
    assert_eq!(
        doc.selection(view_id)
            .primary()
            .cursor(doc.text().slice(..)),
        origin
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stash_review_completion_is_owned_by_its_document_and_opens_frozen_readonly_text(
) -> anyhow::Result<()> {
    use view::features::change_review::HistoricalReviewRequest;
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    let original = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    std::fs::write(&path, "stashed\n")?;
    let repo = vcs::operations::GitRepository::discover(&path)?;
    repo.prepare_stash(true)?.execute().await?;
    let entry = repo.stashes().await?.remove(0);
    let file = repo
        .stash_files(&entry)
        .await?
        .into_iter()
        .find(|file| file.layer() == vcs::operations::StashLayer::Worktree)
        .unwrap();
    let request = HistoricalReviewRequest::stash(&f.editor, repo, entry, file);
    let id = f
        .editor
        .open_historical_review(request.clone(), Action::Replace);
    f.editor.switch(original, Action::Replace);
    finish_review(&mut f, id).await?;
    assert_eq!(
        current_ref!(f.editor).1.id(),
        original,
        "a delayed historical read must never steal focus"
    );
    f.editor.switch(id, Action::Replace);
    f.editor.open_review_source(true, Action::VerticalSplit)?;
    let (view, doc) = current!(f.editor);
    assert_eq!(doc.text().to_string(), "stashed\n");
    assert_eq!(
        doc.workspace_root(),
        stdx::path::canonicalize_existing(path.parent().unwrap())
    );
    assert!(doc.is_content_readonly());
    assert!(doc.path().is_none() && doc.uri().is_none());
    assert!(!doc.is_modified());
    assert!(view::editing::apply(
        doc,
        &editor_core::Transaction::insert(doc.text(), doc.selection(view.id), "edit".into()),
        view.id
    )
    .is_err());
    let canceled = f.editor.open_historical_review(request, Action::Replace);
    assert!(f.editor.close_document(canceled, false).is_ok());
    f.editor.switch(original, Action::Replace);
    // Closed owners cannot be resurrected by either queued or running work.
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    while let Ok(callback) = f.callbacks.try_recv() {
        callback(&mut f.editor);
    }
    assert!(f.editor.document(canceled).is_none());
    assert_eq!(current_ref!(f.editor).1.id(), original);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn historical_buffer_comparisons_keep_captured_unsaved_text_after_refresh(
) -> anyhow::Result<()> {
    use vcs::operations::history::RevisionFile;
    use view::features::change_review::HistoricalReviewRequest;
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    let source = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let repo = vcs::operations::GitRepository::discover(&path)?;
    let commit = repo
        .history_page(repo.history_cursor(None)?.unwrap())
        .await?
        .commits
        .remove(0);
    let endpoint = RevisionFile::new(
        commit.id,
        stdx::path::canonicalize_existing(&path)
            .strip_prefix(repo.root())?
            .to_owned(),
    )?;
    f.replace("captured unsaved\n");
    let request = HistoricalReviewRequest::against_buffer(&f.editor, repo, endpoint, source)?;
    f.replace("newer edits\n");
    let id = f.editor.open_historical_review(request, Action::Replace);
    finish_review(&mut f, id).await?;
    assert!(f
        .editor
        .document(id)
        .unwrap()
        .display_name()
        .starts_with(&format!(
            "{} [diff:",
            path.file_name().unwrap().to_string_lossy()
        )));

    assert!(f
        .editor
        .document(id)
        .unwrap()
        .text()
        .to_string()
        .contains("captured unsaved"));
    assert!(!f
        .editor
        .document(id)
        .unwrap()
        .text()
        .to_string()
        .contains("newer edits"));
    let captured = f.editor.document(id).unwrap().text().clone();
    let other = f.editor.open_review(
        review_request(&f, &path, ReviewComparison::Head),
        Action::VerticalSplit,
        None,
    );
    finish_review(&mut f, other).await?;
    file_event(&mut f, &path, view::file_watcher::EventType::Modified);
    f.configure(|config| {
        config.files.reload.poll.enable = true;
        config.files.reload.poll.interval = 100;
    });
    settle_review_events(&mut f).await;
    assert_eq!(f.editor.document(id).unwrap().text(), &captured);
    f.configure(|config| config.diff.engine = editor_config::DiffEngine::Syntax);
    wait_for_review(&mut f, id, |doc| {
        doc.review().unwrap().engine() == editor_config::DiffEngine::Syntax
    })
    .await?;
    assert!(f
        .editor
        .document(id)
        .unwrap()
        .text()
        .to_string()
        .contains("captured unsaved"));
    assert_eq!(
        f.editor.document(id).unwrap().review().unwrap().engine(),
        editor_config::DiffEngine::Syntax
    );
    assert_eq!(std::fs::read_to_string(path)?, "committed\n");
    assert_eq!(
        f.editor.document(source).unwrap().text().to_string(),
        "newer edits\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn deleted_historical_rows_open_the_before_version_without_recreating_the_file(
) -> anyhow::Result<()> {
    use view::features::change_review::HistoricalReviewRequest;
    let mut f = Fixture::new("deleted contents\n")?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let repo = vcs::operations::GitRepository::discover(&path)?;
    let commit = repo
        .history_page(repo.history_cursor(None)?.unwrap())
        .await?
        .commits
        .remove(0);
    let file = repo
        .commit_files(&commit, 0)
        .await?
        .into_iter()
        .find(|file| {
            file.path()
                == stdx::path::canonicalize_existing(&path)
                    .strip_prefix(repo.root())
                    .unwrap()
        })
        .unwrap();
    let endpoint = file.endpoints(&commit, 0)?.1;
    std::fs::remove_file(&path)?;
    let request = HistoricalReviewRequest::revisions(&f.editor, repo, endpoint, None)?;
    let id = f.editor.open_historical_review(request, Action::Replace);
    finish_review(&mut f, id).await?;
    let (view, doc) = current!(f.editor);
    let row = doc
        .review()
        .unwrap()
        .review()
        .lines
        .iter()
        .position(|row| row.kind == view::features::change_review::ReviewLineKind::Removed)
        .unwrap();
    doc.set_selection(view.id, Selection::point(doc.text().line_to_char_idx(row)));
    f.editor.open_review_source(true, Action::VerticalSplit)?;
    let (_, doc) = current_ref!(f.editor);
    assert_eq!(doc.text().to_string(), "deleted contents\n");
    assert!(doc.is_content_readonly());
    assert!(!path.exists());
    assert!(f.editor.open_review_source(false, Action::Replace).is_err());
    Ok(())
}

#[cfg(feature = "syntax-diff")]
#[tokio::test(flavor = "multi_thread")]
async fn historical_sides_detect_their_own_languages_and_open_with_source_syntax(
) -> anyhow::Result<()> {
    use vcs::operations::history::RevisionFile;
    use view::features::change_review::HistoricalReviewRequest;
    let languages = format!("{JSON_LANGUAGE}\n[[language]]\nname = 'rust'\nscope = 'source.rust'\nfile-types = ['rs']\n");
    let mut f = Fixture::with_languages("{\"old\": 1}\n", &languages)?;
    prepare(&mut f, "initial");
    let old_path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let repo = vcs::operations::GitRepository::discover(&old_path)?;
    let parent = repo
        .history_page(repo.history_cursor(None)?.unwrap())
        .await?
        .commits
        .remove(0);
    // Historical language selection must use each endpoint path.
    let rust_path = f.dir.path().join("source.rs");
    std::fs::write(&rust_path, "fn historical() {}\n")?;
    git(f.dir.path(), &["add", "source.rs"]);
    git(f.dir.path(), &["commit", "-m", "rust source"]);
    let commit = repo
        .history_page(repo.history_cursor(None)?.unwrap())
        .await?
        .commits
        .remove(0);
    f.configure(|config| config.diff.engine = editor_config::DiffEngine::Syntax);
    let before = RevisionFile::new(
        parent.id,
        stdx::path::canonicalize_existing(&old_path)
            .strip_prefix(repo.root())?
            .to_owned(),
    )?;
    let after = RevisionFile::new(commit.id, "source.rs".into())?;
    let request = HistoricalReviewRequest::revisions(&f.editor, repo, Some(before), Some(after))?;
    let id = f.editor.open_historical_review(request, Action::Replace);
    finish_review(&mut f, id).await?;
    assert_eq!(
        f.editor
            .document(id)
            .unwrap()
            .review()
            .unwrap()
            .review()
            .fallback
            .unwrap()
            .0,
        "source languages differ"
    );
    for (kind, language) in [
        (
            view::features::change_review::ReviewLineKind::Removed,
            "review-json",
        ),
        (view::features::change_review::ReviewLineKind::Added, "rust"),
    ] {
        f.editor.switch(id, Action::Replace);
        let (view, doc) = current!(f.editor);
        let state = doc.review().unwrap();
        let row = state
            .review()
            .lines
            .iter()
            .position(|row| row.kind == kind)
            .unwrap();
        assert!(state
            .source()
            .unwrap()
            .syntax_for_row(&state.review().lines[row])
            .is_some());
        doc.set_selection(
            view.id,
            Selection::point(doc.text().line_to_char_idx(row + state.row_offset())),
        );
        f.editor.open_review_source(true, Action::Replace)?;
        let (_, doc) = current_ref!(f.editor);
        assert_eq!(doc.language_name(), Some(language));
        assert!(doc.syntax().is_some());
        let source = doc.id();
        f.editor.set_document_language(source, None)?;
        assert!(f.editor.document(source).unwrap().syntax().is_none());
        f.editor.refresh_doc_language(source);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while f.editor.document(source).unwrap().is_syntax_pending() {
                let callback = f.callbacks.recv().await.unwrap();
                callback(&mut f.editor);
            }
        })
        .await?;
        let doc = f.editor.document(source).unwrap();
        assert_eq!(doc.language_name(), Some(language));
        assert!(doc.syntax().is_some());
        assert!(doc.is_content_readonly());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_a_historical_preview_reuses_its_sources_and_configured_engine(
) -> anyhow::Result<()> {
    use view::features::change_review::HistoricalReviewRequest;
    let mut f = Fixture::new("current\n")?;
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    f.configure(|config| config.diff.engine = editor_config::DiffEngine::Syntax);
    let request = HistoricalReviewRequest::comparison(
        &f.editor,
        path,
        f.dir.path().to_owned(),
        "pinned revision".into(),
        vcs::Comparison {
            before: b"before\n".to_vec(),
            after: b"after\n".to_vec(),
        },
    );
    let mut task = event::TaskController::new();
    let snapshot = request.clone().run(task.restart()).await?;
    let source = snapshot.source.clone();
    let id = f
        .editor
        .open_historical_snapshot(snapshot, Action::VerticalSplit)?;
    finish_review(&mut f, id).await?;
    let state = f.editor.document(id).unwrap().review().unwrap();
    assert!(Arc::ptr_eq(state.source().unwrap(), &source));
    assert_eq!(state.engine(), editor_config::DiffEngine::Syntax);
    assert_eq!(source.after().to_string(), "after\n");
    let canceled = task.restart();
    task.cancel();
    assert!(request.run(canceled).await.is_err());
    Ok(())
}

pub(super) async fn wait_for_review(
    f: &mut Fixture,
    id: view::DocumentId,
    ready: impl Fn(&view::Document) -> bool,
) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let doc = f.editor.document(id).unwrap();
            if !doc.review().unwrap().is_loading() && ready(doc) {
                break;
            }
            f.callbacks
                .recv()
                .await
                .expect("editor callback channel closed")(&mut f.editor);
        }
    })
    .await
    .map_err(|error| {
        anyhow::anyhow!(
            "Review {id:?} did not update: {error}; contents: {}",
            f.editor.document(id).unwrap().text()
        )
    })?;
    Ok(())
}

pub(super) async fn settle_review_events(f: &mut Fixture) {
    let until = tokio::time::sleep(std::time::Duration::from_millis(400));
    tokio::pin!(until);
    loop {
        tokio::select! {
            _ = &mut until => break,
            callback = f.callbacks.recv() => callback.unwrap()(&mut f.editor),
        }
    }
}

pub(super) fn file_event(
    f: &mut Fixture,
    path: &std::path::Path,
    ty: view::file_watcher::EventType,
) {
    use view::file_watcher::{CanonicalPathBuf, Event, Events};
    f.editor.handle_file_events(&Events::from(vec![Event {
        path: CanonicalPathBuf::assert_canonicalized(&stdx::path::canonicalize_existing(path)),
        ty,
    }]));
}

#[tokio::test]
async fn conflict_previews_keep_source_sections_and_explicit_reviews_still_compute_diffs(
) -> anyhow::Result<()> {
    use view::features::change_review::ReviewContent;
    let mut f = Fixture::new("committed\n")?;
    prepare(&mut f, "initial");
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let markers = "prefix\n<<<<<<< HEAD\ncurrent\n=======\nincoming\n>>>>>>> topic\n";
    f.replace(markers);
    let request = review_request(&f, &path, ReviewComparison::Head);
    let preview = request.clone().with_conflict_preview();
    let snapshot = preview.compute_cached(None)?;
    assert!(
        matches!(&snapshot.content, ReviewContent::Conflict(model) if model.regions.len() == 1)
    );
    assert!(snapshot.diff().is_err());
    assert_eq!(snapshot.source.after().to_string(), markers);
    assert!(Arc::ptr_eq(
        &snapshot,
        &preview.compute_cached(Some(snapshot.clone()))?
    ));
    let explicit = request.compute_cached(Some(snapshot))?;
    assert!(explicit.diff()?.additions > 0);
    f.replace("resolved\n");
    let resolved = review_request(&f, &path, ReviewComparison::Head)
        .with_conflict_preview()
        .compute_cached(None)?;
    assert!(matches!(resolved.content, ReviewContent::Diff(_)));
    Ok(())
}

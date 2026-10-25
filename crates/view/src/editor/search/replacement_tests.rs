use super::*;
use crate::editor::{tests::editor, Action};

fn matched(path: &std::path::Path, start: usize, end: usize) -> search::Match {
    search::Match {
        path: path.into(),
        line_start: 0,
        line_end: 0,
        match_start_col: start,
        match_end_col: end,
    }
}

struct TestPlan {
    plan: ReplacementPlan,
    _controller: event::TaskController,
}

impl TestPlan {
    fn apply(self, editor: &mut Editor) -> anyhow::Result<PendingReplacement> {
        self.plan.apply(editor)
    }
}

async fn prepare(
    editor: &Editor,
    matches: &[&search::Match],
    replacement: &str,
    all: bool,
) -> anyhow::Result<TestPlan> {
    let mut task = event::TaskController::new();
    let request = WorkspaceReplacement::new(
        editor,
        search::Search::new("foo", true)?,
        matches,
        replacement,
        all,
        task.restart(),
    )?;
    // Keep cancellation ownership alive while the request prepares.
    let plan = request.prepare().await?;
    Ok(TestPlan {
        plan,
        _controller: task,
    })
}

#[tokio::test]
async fn replacements_write_unopened_files_without_creating_buffers() -> anyhow::Result<()> {
    let (mut editor, _store) = editor("#[|]#\n");
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "foo foo\n")?;
    let documents = editor.documents().count();
    let focus = editor.tree.focus();
    let first = matched(&path, 0, 3);
    let outcome = prepare(&editor, &[&first], "long", false)
        .await?
        .apply(&mut editor)?
        .finish()
        .await;
    assert!(outcome.error.is_none());
    assert_eq!(outcome.count, 1);
    assert_eq!(std::fs::read_to_string(&path)?, "long foo\n");
    assert_eq!(outcome.rows[&path][0].match_start_col, 5);
    assert_eq!(editor.documents().count(), documents);
    assert_eq!(editor.tree.focus(), focus);
    let next = matched(&path, 5, 8);
    let outcome = prepare(&editor, &[&next], "bar", true)
        .await?
        .apply(&mut editor)?
        .finish()
        .await;
    assert!(outcome.error.is_none());
    assert_eq!(std::fs::read_to_string(&path)?, "long bar\n");
    assert_eq!(editor.documents().count(), documents);
    Ok(())
}

#[tokio::test]
async fn open_buffers_remain_unsaved_and_have_independent_undo() -> anyhow::Result<()> {
    let (mut editor, _store) = editor("#[|]#\n");
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "foo\n")?;
    let id = editor.open(&path, Action::Replace)?;
    editor.enter_insert_mode();
    editor.insert_char('α')?;
    let hidden = editor.new_file(Action::Replace);
    let hidden_text = editor.document(hidden).unwrap().text().clone();
    let focus = editor.tree.focus();
    let outcome = prepare(&editor, &[&matched(&path, 1, 4)], "long", false)
        .await?
        .apply(&mut editor)?
        .finish()
        .await;
    assert!(outcome.error.is_none());
    assert_eq!(editor.document(id).unwrap().text(), "αlong\n");
    assert!(editor.document(id).unwrap().is_modified());
    assert_eq!(editor.document(hidden).unwrap().text(), &hidden_text);
    assert_eq!(editor.tree.focus(), focus);
    assert_eq!(std::fs::read_to_string(&path)?, "foo\n");
    editor.open(&path, Action::Replace)?;
    assert!(editor.change_history(buffer::HistoryAction::Undo));
    assert_eq!(editor.document(id).unwrap().text(), "αfoo\n");
    assert!(editor.change_history(buffer::HistoryAction::Undo));
    assert_eq!(editor.document(id).unwrap().text(), "foo\n");
    Ok(())
}

#[tokio::test]
async fn replace_all_deduplicates_files_and_preserves_other_files() -> anyhow::Result<()> {
    let (mut editor, _store) = editor("#[|]#\n");
    let dir = tempfile::tempdir()?;
    let paths = ["one", "two", "excluded"].map(|name| dir.path().join(name));
    for path in &paths {
        std::fs::write(path, "foo foo\n")?;
    }
    let a = matched(&paths[0], 0, 3);
    let b = matched(&paths[1], 0, 3);
    let alias = matched(&dir.path().join("sub/../one"), 0, 3);
    let outcome = prepare(&editor, &[&a, &a, &alias, &b], "bar", true)
        .await?
        .apply(&mut editor)?
        .finish()
        .await;
    assert!(outcome.error.is_none());
    assert_eq!(outcome.count, 4);
    assert_eq!(editor.documents().count(), 1);
    for path in &paths[..2] {
        assert_eq!(std::fs::read_to_string(path)?, "bar bar\n");
    }
    assert_eq!(std::fs::read_to_string(&paths[2])?, "foo foo\n");
    Ok(())
}

#[tokio::test]
async fn stale_or_newly_opened_buffers_reject_the_plan_before_writes() -> anyhow::Result<()> {
    let (mut editor, _store) = editor("#[|]#\n");
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "foo\n")?;
    let selected = matched(&path, 0, 3);
    let plan = prepare(&editor, &[&selected], "bar", true).await?;
    let id = editor.open(&path, Action::Replace)?;
    assert!(plan.apply(&mut editor).is_err());
    let plan = prepare(&editor, &[&selected], "bar", true).await?;
    editor.enter_insert_mode();
    editor.insert_char('x')?;
    assert!(plan.apply(&mut editor).is_err());
    assert_eq!(editor.document(id).unwrap().text(), "xfoo\n");
    assert_eq!(std::fs::read_to_string(&path)?, "foo\n");
    Ok(())
}

#[tokio::test]
async fn pending_writes_prevent_opening_the_target_and_preserve_external_changes(
) -> anyhow::Result<()> {
    let (mut editor, _store) = editor("#[|]#\n");
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "foo\n")?;
    let pending = prepare(&editor, &[&matched(&path, 0, 3)], "bar", true)
        .await?
        .apply(&mut editor)?;
    assert!(editor.open(&path, Action::Load).is_err());
    std::fs::write(&path, "external edit\n")?;
    let outcome = pending.finish().await;
    assert!(outcome.error.is_some());
    assert_eq!(outcome.count, 0);
    assert_eq!(std::fs::read_to_string(&path)?, "external edit\n");
    assert!(editor.open(&path, Action::Load).is_ok());
    Ok(())
}

#[tokio::test]
async fn invalid_later_buffer_and_cancellation_prevent_any_replacement() -> anyhow::Result<()> {
    let (mut editor, _store) = editor("#[|]#\n");
    let dir = tempfile::tempdir()?;
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    std::fs::write(&first, "foo\n")?;
    std::fs::write(&second, "foo\n")?;
    let id = editor.open(&first, Action::Load)?;
    let mut doc = Document::from(
        Rope::from("foo\n"),
        None,
        editor.config.clone(),
        editor.syn_loader.load_full(),
    );
    doc.set_path(Some(&second));
    editor.new_file_from_document(Action::Replace, doc.with_readonly_name("source".into()));
    assert!(prepare(
        &editor,
        &[&matched(&first, 0, 3), &matched(&second, 0, 3)],
        "bar",
        true
    )
    .await
    .is_err());
    let mut task = event::TaskController::new();
    let request = WorkspaceReplacement::new(
        &editor,
        search::Search::new("foo", true)?,
        &[&matched(&first, 0, 3)],
        "bar",
        true,
        task.restart(),
    )?;
    let plan = request.prepare().await?;
    task.cancel();
    assert!(plan.apply(&mut editor).is_err());
    assert_eq!(editor.document(id).unwrap().text(), "foo\n");
    assert_eq!(std::fs::read_to_string(&first)?, "foo\n");
    Ok(())
}

#[tokio::test]
async fn disk_replacement_preserves_bom_line_endings_and_permissions() -> anyhow::Result<()> {
    let (mut editor, _store) = editor("#[|]#\n");
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "\u{feff}foo\r\n")?;
    let permissions = std::fs::metadata(&path)?.permissions();
    let outcome = prepare(&editor, &[&matched(&path, 0, 3)], "α", true)
        .await?
        .apply(&mut editor)?
        .finish()
        .await;
    assert!(outcome.error.is_none());
    assert_eq!(std::fs::read_to_string(&path)?, "\u{feff}α\r\n");
    assert_eq!(std::fs::metadata(&path)?.permissions(), permissions);
    Ok(())
}

#[tokio::test]
async fn accepted_writes_finish_after_cancellation_and_report_partial_failures(
) -> anyhow::Result<()> {
    let (mut editor, _store) = editor("#[|]#\n");
    let dir = tempfile::tempdir()?;
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    std::fs::write(&first, "foo\n")?;
    std::fs::write(&second, "foo\n")?;
    let mut task = event::TaskController::new();
    let request = WorkspaceReplacement::new(
        &editor,
        search::Search::new("foo", true)?,
        &[&matched(&first, 0, 3), &matched(&second, 0, 3)],
        "bar",
        true,
        task.restart(),
    )?;
    let pending = request.prepare().await?.apply(&mut editor)?;
    task.cancel();
    std::fs::write(&second, "external contents\n")?;
    let outcome = pending.finish().await;
    assert!(outcome.error.is_some());
    assert_eq!(outcome.count, 1);
    assert!(outcome.rows.contains_key(&first));
    assert!(!outcome.rows.contains_key(&second));
    assert_eq!(std::fs::read_to_string(&first)?, "bar\n");
    assert_eq!(std::fs::read_to_string(&second)?, "external contents\n");
    assert_eq!(editor.documents().count(), 1);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn disk_replacement_preserves_symlinks_and_reserves_the_resolved_path() -> anyhow::Result<()>
{
    let (mut editor, _store) = editor("#[|]#\n");
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("target.txt");
    let link = dir.path().join("link.txt");
    std::fs::write(&target, "foo\n")?;
    std::os::unix::fs::symlink("target.txt", &link)?;
    let pending = prepare(&editor, &[&matched(&link, 0, 3)], "bar", true)
        .await?
        .apply(&mut editor)?;
    assert!(editor.open(&target, Action::Load).is_err());
    let outcome = pending.finish().await;
    assert!(outcome.error.is_none());
    assert!(std::fs::symlink_metadata(&link)?.file_type().is_symlink());
    assert_eq!(std::fs::read_to_string(&target)?, "bar\n");
    assert_eq!(std::fs::read_to_string(&link)?, "bar\n");
    assert_eq!(editor.documents().count(), 1);
    Ok(())
}

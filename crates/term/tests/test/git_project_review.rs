use super::helpers::{run_event_loop_until_idle, AppBuilder};
use std::{path::Path, process::Command, time::Duration};
use stdx::rope::RopeExt as _;
use term::application::Application;
use tokio_stream::wrappers::UnboundedReceiverStream;
use ui_core::input::parse_macro;
use view::features::change_review::ReviewLineKind;
use view::{current_ref, editor::Action};

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_remove("GIT_DIR")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repo() -> anyhow::Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    git(dir.path(), &["init"]);
    std::fs::write(dir.path().join(".gitattributes"), "* text eol=lf\n")?;
    std::fs::write(dir.path().join("a.txt"), "head\n")?;
    std::fs::write(dir.path().join("b.txt"), "other\n")?;
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "initial"]);
    Ok(dir)
}

async fn keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in parse_macro(keys)? {
        tx.send(Ok(Event::Key(KeyEvent::from(key))))?;
    }
    let mut input = UnboundedReceiverStream::new(rx);
    assert!(app.event_loop_until_idle(&mut input).await);
    Ok(())
}

async fn ready(app: &mut Application) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(app).await;
            if current_ref!(app.editor)
                .1
                .project_review()
                .is_some_and(|state| !state.loading)
            {
                break;
            }
        }
    })
    .await?;
    Ok(())
}

async fn acted(app: &mut Application) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(app).await;
            if !app.editor.handlers().git_operations.busy()
                && current_ref!(app.editor)
                    .1
                    .project_review()
                    .is_some_and(|state| !state.loading)
            {
                break;
            }
        }
    })
    .await?;
    Ok(())
}

async fn ready_file_review(app: &mut Application) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(app).await;
            if !app.editor.handlers().git_operations.busy()
                && current_ref!(app.editor)
                    .1
                    .review()
                    .is_some_and(|review| !review.is_loading())
            {
                break;
            }
        }
    })
    .await?;
    Ok(())
}

fn row(app: &Application, file: &str, kind: ReviewLineKind, text: &str) -> usize {
    let project = current_ref!(app.editor).1.project_review().unwrap();
    let section = project
        .sections
        .iter()
        .find(|section| section.status.path().ends_with(file))
        .unwrap();
    section.start_line
        + 1
        + section
            .snapshot
            .as_ref()
            .unwrap()
            .review
            .lines
            .iter()
            .position(|row| row.kind == kind && row.text.contains(text))
            .unwrap()
}

fn select_line(app: &mut Application, line: usize) {
    let (view, doc) = view::current!(app.editor);
    doc.set_selection(
        view.id,
        editor_core::Selection::point(doc.text().line_to_char_idx(line)),
    );
}

fn index_text(root: &Path, file: &str) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", &format!(":{file}")])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn project_review_separates_layers_refreshes_and_opens_file_reviews() -> anyhow::Result<()> {
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "staged\n")?;
    git(dir.path(), &["add", "a.txt"]);
    std::fs::write(&path, "disk\n")?;
    std::fs::write(dir.path().join("new.txt"), "untracked\n")?;
    std::fs::write(dir.path().join("binary.bin"), b"\0binary")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    run_event_loop_until_idle(&mut app).await;
    keys(&mut app, "Ibuffer-<esc>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "buffer-disk\n"
    );
    keys(&mut app, ":vcs-project-diff<ret>").await?;
    ready(&mut app).await?;
    let id = current_ref!(app.editor).1.id();
    let text = current_ref!(app.editor).1.text().to_string();
    assert!(text.contains("-staged"), "{text}");
    assert!(text.contains("+buffer-disk"), "{text}");
    assert!(text.contains("new.txt"));
    assert!(text.contains("binary.bin") && text.contains("Cannot review:"));
    assert!(current_ref!(app.editor).1.is_content_readonly());
    keys(&mut app, "iSHOULD-NOT-EDIT<esc>").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), text);
    assert_eq!(std::fs::read_to_string(&path)?, "disk\n");
    keys(&mut app, ":vcs-project-next file<ret><ret>").await?;
    assert!(current_ref!(app.editor).1.review().is_some());
    keys(&mut app, "<C-o>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), id);
    std::fs::write(dir.path().join("new.txt"), "refreshed\n")?;
    keys(&mut app, ":vcs-project-refresh<ret>").await?;
    ready(&mut app).await?;
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .contains("+refreshed"));
    keys(&mut app, ":vcs-project-diff staged<ret>").await?;
    ready(&mut app).await?;
    let text = current_ref!(app.editor).1.text().to_string();
    assert!(text.contains("-head") && text.contains("+staged"), "{text}");
    assert!(!text.contains("new.txt") && !text.contains("buffer-disk"));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn project_review_includes_clean_files_with_unsaved_changes_and_cancels_closed_work(
) -> anyhow::Result<()> {
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    run_event_loop_until_idle(&mut app).await;
    let source = current_ref!(app.editor).1.id();
    keys(&mut app, "Iunsaved-<esc>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "unsaved-head\n"
    );
    keys(&mut app, ":vcs-project-diff<ret>").await?;
    ready(&mut app).await?;
    let review = current_ref!(app.editor).1.id();
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .contains("+unsaved-head"));
    assert_eq!(std::fs::read_to_string(&path)?, "head\n");
    keys(&mut app, ":vcs-project-refresh<ret>").await?;
    app.editor.switch(source, Action::Replace);
    app.editor
        .close_document(review, false)
        .map_err(|_| anyhow::anyhow!("close failed"))?;
    run_event_loop_until_idle(&mut app).await;
    assert!(app.editor.document(review).is_none());
    git(dir.path(), &["diff", "--exit-code"]);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_staging_keeps_index_disk_buffer_layers_and_bom_separate() -> anyhow::Result<()> {
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "\u{feff}head\n")?;
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-m", "BOM baseline"]);
    std::fs::write(&path, "\u{feff}staged\n")?;
    git(dir.path(), &["add", "a.txt"]);
    std::fs::write(&path, "\u{feff}disk\n")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let source = current_ref!(app.editor).1.id();
    keys(&mut app, "Iunsaved-<esc>:vcs-project-diff buffer<ret>").await?;
    ready(&mut app).await?;
    let selected = row(&app, "a.txt", ReviewLineKind::Added, "unsaved-disk");
    select_line(&mut app, selected);
    keys(&mut app, ":vcs-stage-hunk<ret>").await?;
    acted(&mut app).await?;
    assert_eq!(index_text(dir.path(), "a.txt"), "\u{feff}unsaved-disk\n");
    assert_eq!(std::fs::read_to_string(&path)?, "\u{feff}disk\n");
    assert_eq!(
        app.editor.document(source).unwrap().text().to_string(),
        "unsaved-disk\n"
    );
    keys(&mut app, ":vcs-project-diff staged<ret>").await?;
    ready(&mut app).await?;
    let selected = row(&app, "a.txt", ReviewLineKind::Added, "unsaved-disk");
    select_line(&mut app, selected);
    keys(&mut app, ":vcs-unstage-hunk<ret>").await?;
    acted(&mut app).await?;
    assert_eq!(index_text(dir.path(), "a.txt"), "\u{feff}head\n");
    assert_eq!(std::fs::read_to_string(&path)?, "\u{feff}disk\n");
    assert!(app.editor.close_document(source, true).is_ok());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_selected_deleted_rows_stage_and_unstage_independently() -> anyhow::Result<()> {
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    let original = "one\nremove first\nremove second\nlast\n";
    std::fs::write(&path, original)?;
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-m", "deletion baseline"]);
    std::fs::write(&path, "one\nlast\n")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    keys(&mut app, ":vcs-project-diff unstaged<ret>").await?;
    ready(&mut app).await?;
    let selected = row(&app, "a.txt", ReviewLineKind::Removed, "remove first");
    select_line(&mut app, selected);
    keys(&mut app, ":vcs-stage-lines<ret>").await?;
    acted(&mut app).await?;
    assert_eq!(
        index_text(dir.path(), "a.txt"),
        "one\nremove second\nlast\n"
    );
    keys(&mut app, ":vcs-project-diff staged<ret>").await?;
    ready(&mut app).await?;
    let selected = row(&app, "a.txt", ReviewLineKind::Removed, "remove first");
    select_line(&mut app, selected);
    keys(&mut app, ":vcs-unstage-lines<ret>").await?;
    acted(&mut app).await?;
    assert_eq!(index_text(dir.path(), "a.txt"), original);
    assert_eq!(std::fs::read_to_string(&path)?, "one\nlast\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_partial_actions_reject_cross_file_and_stale_buffer_selections(
) -> anyhow::Result<()> {
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "changed a\n")?;
    std::fs::write(dir.path().join("b.txt"), "changed b\n")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let source = current_ref!(app.editor).1.id();
    keys(&mut app, ":vcs-project-diff buffer<ret>").await?;
    ready(&mut app).await?;
    let first = row(&app, "a.txt", ReviewLineKind::Added, "changed a");
    let second = row(&app, "b.txt", ReviewLineKind::Added, "changed b");
    let (view, doc) = view::current!(app.editor);
    doc.set_selection(
        view.id,
        editor_core::Selection::single(
            doc.text().line_to_char_idx(first),
            doc.text().line_to_char_idx(second) + 1,
        ),
    );
    keys(&mut app, ":vcs-stage-lines<ret>").await?;
    assert!(app
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("multiple files"));
    assert!(!app.editor.handlers().git_operations.busy());
    select_line(&mut app, first);
    let source_doc = app.editor.document(source).unwrap();
    let change = editor_core::Transaction::change(
        source_doc.text(),
        [(0, 0, Some("newer-".into()))].into_iter(),
    );
    app.editor
        .apply_document_changes(source, source_doc.version(), change.changes())?;
    keys(&mut app, ":vcs-stage-hunk<ret>").await?;
    assert!(app
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("Source buffer changed"));
    assert_eq!(index_text(dir.path(), "a.txt"), "head\n");
    assert_eq!(index_text(dir.path(), "b.txt"), "other\n");
    assert!(app.editor.close_document(source, true).is_ok());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_change_motions_and_stage_next_follow_refreshed_hunks_and_files(
) -> anyhow::Result<()> {
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    let original = "first\n0\n1\n2\n3\n4\n5\n6\n7\n8\nlast\n";
    std::fs::write(&path, original)?;
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-m", "separate hunks"]);
    std::fs::write(
        &path,
        original.replace("first", "FIRST").replace("last", "LAST"),
    )?;
    std::fs::write(dir.path().join("b.txt"), "changed b\n")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    keys(&mut app, ":vcs-project-diff buffer<ret>").await?;
    ready(&mut app).await?;
    let hunks = current_ref!(app.editor)
        .1
        .project_review()
        .unwrap()
        .hunk_rows();
    assert_eq!(hunks.len(), 3);
    keys(&mut app, "[G2]g").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.cursor_line(view.id), hunks[2]);
    keys(&mut app, "[G:vcs-stage-and-next<ret>").await?;
    acted(&mut app).await?;
    let (view, doc) = current_ref!(app.editor);
    let project = doc.project_review().unwrap();
    let section = project.section(doc.cursor_line(view.id)).unwrap();
    assert!(section.status.path().ends_with("a.txt"));
    assert_eq!(
        section.row(doc.cursor_line(view.id)).unwrap().kind,
        ReviewLineKind::Header
    );
    assert_eq!(
        index_text(dir.path(), "a.txt"),
        original.replace("first", "FIRST")
    );
    keys(&mut app, ":vcs-stage-and-next<ret>").await?;
    acted(&mut app).await?;
    let (view, doc) = current_ref!(app.editor);
    assert!(doc
        .project_review()
        .unwrap()
        .section(doc.cursor_line(view.id))
        .unwrap()
        .status
        .path()
        .ends_with("b.txt"));
    assert_eq!(
        index_text(dir.path(), "a.txt"),
        original.replace("first", "FIRST").replace("last", "LAST")
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_stage_next_keeps_a_newer_cursor_movement() -> anyhow::Result<()> {
    use view::features::git_operations::partial;
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "changed a\n")?;
    std::fs::write(dir.path().join("b.txt"), "changed b\n")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    keys(&mut app, ":vcs-project-diff buffer<ret>").await?;
    ready(&mut app).await?;
    let first = row(&app, "a.txt", ReviewLineKind::Added, "changed a");
    select_line(&mut app, first);
    let (request, next) = partial::capture(
        &app.editor,
        vcs::operations::FileOperation::Stage,
        false,
        false,
    )?;
    let completion = request
        .prepare()
        .await?
        .admit(&mut app.editor)?
        .execute()
        .await;
    assert!(completion.succeeded());
    let line = completion.next_partial_line();
    completion.finish(&mut app.editor);
    next.unwrap().select_after(&mut app.editor, line);
    select_line(&mut app, 0);
    ready(&mut app).await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.cursor_line(view.id), 0);
    assert_eq!(index_text(dir.path(), "a.txt"), "changed a\n");
    assert_eq!(index_text(dir.path(), "b.txt"), "other\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_staged_actions_recheck_head_after_async_preflight() -> anyhow::Result<()> {
    use view::features::git_operations::partial;
    let dir = repo()?;
    git(
        dir.path(),
        &["commit", "--allow-empty", "-m", "second commit"],
    );
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "staged a\n")?;
    git(dir.path(), &["add", "a.txt"]);
    let index = std::fs::read(dir.path().join(".git/index"))?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    keys(&mut app, ":vcs-project-diff staged<ret>").await?;
    ready(&mut app).await?;
    let selected = row(&app, "a.txt", ReviewLineKind::Added, "staged a");
    select_line(&mut app, selected);
    let (request, _) = partial::capture(
        &app.editor,
        vcs::operations::FileOperation::Unstage,
        true,
        false,
    )?;
    git(dir.path(), &["update-ref", "HEAD", "HEAD^"]);
    let prepared = request.prepare().await?;
    assert!(prepared.admit(&mut app.editor).is_err());
    assert!(!app.editor.handlers().git_operations.busy());
    assert_eq!(std::fs::read(dir.path().join(".git/index"))?, index);
    assert_eq!(index_text(dir.path(), "a.txt"), "staged a\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_staged_review_rechecks_head_before_unstaging() -> anyhow::Result<()> {
    use view::features::git_operations::partial;
    let dir = repo()?;
    git(
        dir.path(),
        &["commit", "--allow-empty", "-m", "second commit"],
    );
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "staged a\n")?;
    git(dir.path(), &["add", "a.txt"]);
    let index = std::fs::read(dir.path().join(".git/index"))?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    keys(&mut app, ":diff staged<ret>").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(&mut app).await;
            if current_ref!(app.editor)
                .1
                .review()
                .is_some_and(|review| !review.is_loading())
            {
                break;
            }
        }
    })
    .await?;
    let (_, doc) = current_ref!(app.editor);
    let selected = (0..doc.text().len_lines(stdx::rope::LINE_TYPE))
        .find(|line| {
            doc.review()
                .unwrap()
                .row(*line)
                .is_some_and(|row| row.kind == ReviewLineKind::Added)
        })
        .unwrap();
    select_line(&mut app, selected);
    let (request, _) = partial::capture(
        &app.editor,
        vcs::operations::FileOperation::Unstage,
        true,
        false,
    )?;
    git(dir.path(), &["update-ref", "HEAD", "HEAD^"]);
    let prepared = request.prepare().await?;
    assert!(prepared.admit(&mut app.editor).is_err());
    assert!(partial::capture(
        &app.editor,
        vcs::operations::FileOperation::Unstage,
        true,
        false
    )
    .is_err());
    assert_eq!(std::fs::read(dir.path().join(".git/index"))?, index);
    assert_eq!(index_text(dir.path(), "a.txt"), "staged a\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_review_of_an_unopened_bom_file_uses_its_raw_disk_source() -> anyhow::Result<()> {
    for comparison in ["buffer", "head"] {
        let dir = repo()?;
        let path = dir.path().join("bom.txt");
        std::fs::write(&path, "\u{feff}first\n")?;
        let mut app = AppBuilder::new()
            .with_file(dir.path().join("a.txt"), None)
            .build()?;
        keys(&mut app, ":vcs-project-diff buffer<ret>").await?;
        ready(&mut app).await?;
        let selected = row(&app, "bom.txt", ReviewLineKind::Added, "first");
        select_line(&mut app, selected);
        keys(&mut app, "<ret>").await?;
        ready_file_review(&mut app).await?;
        if comparison == "head" {
            keys(&mut app, ":diff head<ret>").await?;
            ready_file_review(&mut app).await?;
        }
        assert!(!app
            .editor
            .documents()
            .any(|doc| doc.path().is_some_and(|path| path.ends_with("bom.txt"))));
        let (_, doc) = current_ref!(app.editor);
        let selected = (0..doc.text().len_lines(stdx::rope::LINE_TYPE))
            .find(|line| {
                doc.review()
                    .unwrap()
                    .row(*line)
                    .is_some_and(|row| row.kind == ReviewLineKind::Added)
            })
            .unwrap();
        select_line(&mut app, selected);
        keys(&mut app, ":vcs-stage-hunk<ret>").await?;
        ready_file_review(&mut app).await?;
        assert_eq!(
            index_text(dir.path(), "bom.txt"),
            "\u{feff}first\n",
            "{comparison}"
        );
        assert_eq!(std::fs::read_to_string(&path)?, "\u{feff}first\n");
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

use super::helpers::{run_event_loop_until_idle as run_until_idle, AppBuilder};
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

async fn run_event_loop_until_idle(app: &mut Application) {
    app.editor.reset_idle_timer();
    run_until_idle(app).await;
    tokio::task::yield_now().await;
}

async fn keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for key in parse_macro(keys)? {
        tx.send(Ok(Event::Key(KeyEvent::from(key))))?;
    }
    let mut input = UnboundedReceiverStream::new(rx);
    app.editor.reset_idle_timer();
    assert!(app.event_loop_until_idle(&mut input).await);
    Ok(())
}

async fn ready(app: &mut Application) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(app).await;
            tokio::task::yield_now().await;
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
            tokio::task::yield_now().await;
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
            tokio::task::yield_now().await;
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
            .diff()
            .unwrap()
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
    let source = current_ref!(app.editor).1.id();
    keys(&mut app, ":vcs-project-diff<ret>").await?;
    ready(&mut app).await?;
    assert_eq!(
        current_ref!(app.editor)
            .1
            .project_review()
            .unwrap()
            .sections
            .iter()
            .find(|section| section.status.path().ends_with("a.txt"))
            .and_then(|section| section.source_document()),
        Some(source)
    );
    let id = current_ref!(app.editor).1.id();
    let text = current_ref!(app.editor).1.text().to_string();
    assert!(text.contains("-staged"), "{text}");
    assert!(
        text.contains("+disk") && !text.contains("+buffer-disk"),
        "{text}"
    );
    assert!(
        text.contains("Staged · HEAD") && text.contains("Unstaged · index"),
        "{text}"
    );
    assert_eq!(
        current_ref!(app.editor)
            .1
            .project_review()
            .unwrap()
            .sections
            .iter()
            .filter(|section| section.status.path().ends_with("a.txt"))
            .count(),
        2
    );
    assert!(text.contains("new.txt"));
    assert!(text.contains("binary.bin") && text.contains("Cannot review:"));
    assert!(current_ref!(app.editor).1.is_content_readonly());
    keys(&mut app, "iSHOULD-NOT-EDIT<esc>").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), text);
    assert_eq!(std::fs::read_to_string(&path)?, "disk\n");
    let heading = current_ref!(app.editor)
        .1
        .project_review()
        .unwrap()
        .sections[0]
        .start_line;
    select_line(&mut app, heading);
    keys(&mut app, "<ret>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), source);
    assert!(!current_ref!(app.editor).1.is_content_readonly());
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
    assert!(!current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .contains("+unsaved-head"));
    keys(&mut app, "<A-b>").await?;
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
        keys(&mut app, "<A-o>").await?;
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

#[tokio::test(flavor = "multi_thread")]
async fn git_review_contextual_actions_fold_files_and_keep_comparisons_separate(
) -> anyhow::Result<()> {
    use view::features::change_review::ReviewComparison;
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    let original = "first\n0\n1\n2\n3\n4\n5\n6\n7\n8\nlast\n";
    std::fs::write(&path, original)?;
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-m", "hunks"]);
    std::fs::write(
        &path,
        original.replace("first", "FIRST").replace("last", "LAST"),
    )?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    keys(&mut app, ":vcs-project-diff<ret>").await?;
    ready(&mut app).await?;
    let heading = current_ref!(app.editor)
        .1
        .project_review()
        .unwrap()
        .sections[0]
        .start_line;
    select_line(&mut app, heading);
    keys(&mut app, "<tab>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.folded_ranges(view.id).len(), 1);
    assert_eq!(doc.cursor_line(view.id), heading);
    keys(&mut app, "<tab>]g<tab>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.folded_ranges(view.id).len(), 1);
    // A folded hunk stages just that hunk, then advances within Unstaged.
    keys(&mut app, "s").await?;
    acted(&mut app).await?;
    assert_eq!(
        index_text(dir.path(), "a.txt"),
        original.replace("first", "FIRST")
    );
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.project_review()
            .unwrap()
            .section(doc.cursor_line(view.id))
            .unwrap()
            .comparison,
        ReviewComparison::Unstaged
    );
    let unstaged = row(&app, "a.txt", ReviewLineKind::Added, "LAST");
    let staged = current_ref!(app.editor)
        .1
        .project_review()
        .unwrap()
        .sections
        .iter()
        .find(|section| section.comparison == ReviewComparison::Staged)
        .unwrap()
        .start_line;
    let (view, doc) = view::current!(app.editor);
    doc.set_selection(
        view.id,
        editor_core::Selection::new(
            vec![
                editor_core::Range::point(doc.text().line_to_char_idx(unstaged)),
                editor_core::Range::point(doc.text().line_to_char_idx(staged)),
            ]
            .into(),
            0,
        ),
    );
    keys(&mut app, "s").await?;
    assert!(app.editor.get_status().unwrap().0.contains("comparisons"));
    assert_eq!(
        index_text(dir.path(), "a.txt"),
        original.replace("first", "FIRST")
    );
    select_line(&mut app, staged);
    keys(&mut app, "u").await?;
    acted(&mut app).await?;
    assert_eq!(index_text(dir.path(), "a.txt"), original);
    // The file header stages both remaining hunks, including while folded.
    let heading = current_ref!(app.editor)
        .1
        .project_review()
        .unwrap()
        .sections[0]
        .start_line;
    select_line(&mut app, heading);
    keys(&mut app, "<tab>s").await?;
    acted(&mut app).await?;
    assert_eq!(
        index_text(dir.path(), "a.txt"),
        original.replace("first", "FIRST").replace("last", "LAST")
    );
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .contains("Unstaged · index → saved files (0 files)"));
    assert_eq!(
        std::fs::read_to_string(&path)?,
        original.replace("first", "FIRST").replace("last", "LAST")
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn git_review_selected_lines_and_buffer_file_actions_preserve_unsaved_text(
) -> anyhow::Result<()> {
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    keys(&mut app, "ggIone\ntwo\n<esc>:vcs-project-diff buffer<ret>").await?;
    ready(&mut app).await?;
    let source = app.editor.document_by_path(&path).unwrap().id();
    let first = row(&app, "a.txt", ReviewLineKind::Added, "one");
    let (view, doc) = view::current!(app.editor);
    let start = doc.text().line_to_char_idx(first);
    doc.set_selection(view.id, editor_core::Selection::single(start, start + 3));
    keys(&mut app, "s").await?;
    acted(&mut app).await?;
    assert_eq!(index_text(dir.path(), "a.txt"), "one\nhead\n");
    let heading = current_ref!(app.editor)
        .1
        .project_review()
        .unwrap()
        .sections[0]
        .start_line;
    select_line(&mut app, heading);
    keys(&mut app, "s").await?;
    acted(&mut app).await?;
    assert_eq!(index_text(dir.path(), "a.txt"), "one\ntwo\nhead\n");
    assert_eq!(std::fs::read_to_string(&path)?, "head\n");
    assert_eq!(
        app.editor.document(source).unwrap().text().to_string(),
        "one\ntwo\nhead\n"
    );
    assert!(app.editor.close_document(source, true).is_ok());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn git_review_refreshes_external_index_disk_and_opted_in_buffer_changes() -> anyhow::Result<()>
{
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "disk\n")?;
    let mut config = super::helpers::test_config();
    config.editor.files.watch.enable = true;
    config.editor.files.watch.watch_vcs = true;
    config.editor.files.watch.hidden = false;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&path, None)
        .build()?;
    keys(&mut app, ":vcs-project-diff<ret>").await?;
    ready(&mut app).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !app.editor.file_watcher.is_watching(&path) {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    std::fs::write(&path, "external\n")?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(&mut app).await;
            if current_ref!(app.editor)
                .1
                .text()
                .to_string()
                .contains("+external")
            {
                break;
            }
        }
    })
    .await?;
    git(dir.path(), &["add", "a.txt"]);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(&mut app).await;
            let text = current_ref!(app.editor).1.text().to_string();
            if text.contains("+external")
                && text.contains("Unstaged · index → saved files (0 files)")
            {
                break;
            }
        }
    })
    .await
    .map_err(|error| {
        anyhow::anyhow!(
            "external refresh: {error}; {}",
            current_ref!(app.editor).1.text()
        )
    })?;
    keys(&mut app, "<A-b>").await?;
    ready(&mut app).await?;
    let source = app.editor.document_by_path(&path).unwrap().id();
    let doc = app.editor.document(source).unwrap();
    let transaction =
        editor_core::Transaction::change(doc.text(), [(0, 0, Some("buffer-".into()))].into_iter());
    app.editor
        .apply_document_changes(source, doc.version(), transaction.changes())?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(&mut app).await;
            if current_ref!(app.editor)
                .1
                .text()
                .to_string()
                .contains("+buffer-")
            {
                break;
            }
        }
    })
    .await
    .map_err(|error| {
        anyhow::anyhow!(
            "buffer refresh: {error}; {}",
            current_ref!(app.editor).1.text()
        )
    })?;
    assert!(app.editor.close_document(source, true).is_ok());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn git_review_bindings_are_remappable_and_visible_while_scrolling() -> anyhow::Result<()> {
    let dir = repo()?;
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "changed\n")?;
    let mut config = super::helpers::test_config();
    config.component_keys = term::config::Config::load(
        Ok(
            &"[keys.project-review]\ns = 'unbound'\n[ keys.project-review.z ]\nx = 'stage'\n"
                .to_owned(),
        ),
        Err(term::config::ConfigLoadError::default()),
    )
    .unwrap()
    .component_keys;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&path, None)
        .build()?;
    keys(&mut app, " gr").await?;
    ready(&mut app).await?;
    keys(&mut app, "").await?;
    let screen: String = app
        .terminal_backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("zx stage"), "{screen}");
    let heading = current_ref!(app.editor)
        .1
        .project_review()
        .unwrap()
        .sections[0]
        .start_line;
    select_line(&mut app, heading);
    keys(&mut app, "z<esc>").await?;
    assert_eq!(index_text(dir.path(), "a.txt"), "head\n");
    keys(&mut app, "zx").await?;
    acted(&mut app).await?;
    assert_eq!(index_text(dir.path(), "a.txt"), "changed\n");
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.cursor_line(view.id),
        4,
        "Stay at the empty Unstaged section"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn git_review_renders_both_groups_with_native_syntax_and_diff_styles() -> anyhow::Result<()> {
    let dir = repo()?;
    let path = dir.path().join("source.rs");
    std::fs::write(&path, "fn original() { 1 }\n")?;
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "source"]);
    std::fs::write(&path, "fn staged() { 2 }\n")?;
    git(dir.path(), &["add", "source.rs"]);
    std::fs::write(&path, "fn edited() { 3 }\n")?;
    let mut app = AppBuilder::new()
        .with_lang_loader(super::helpers::test_syntax_loader(None))
        .with_file(&path, None)
        .build()?;
    app.editor
        .set_theme(theme::Theme::from(toml::from_str::<toml::Value>(
            r#"
        "ui.text" = "white"
        "ui.text.focus" = { fg = "yellow", modifiers = ["bold"] }
        "ui.selection" = { bg = "blue" }
        "ui.cursor" = { bg = "yellow" }
        "ui.statusline" = { fg = "white", bg = "black" }
        "keyword" = "magenta"
        "function" = "cyan"
        "constant.numeric" = "blue"
        "diff.plus" = "green"
        "diff.minus" = "red"
        "diff.plus.line" = { bg = "black" }
        "diff.minus.line" = { bg = "black" }
    "#,
        )?))?;
    keys(&mut app, " gr").await?;
    ready(&mut app).await?;
    keys(&mut app, "").await?;
    let buffer = app.terminal_backend().buffer();
    let screen: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(
        screen.contains("Unstaged · index") && screen.contains("Staged · HEAD"),
        "{screen}"
    );
    assert!(
        screen.contains("s stage") && screen.contains("u unstage"),
        "{screen}"
    );
    assert!(buffer
        .content
        .iter()
        .any(|cell| cell.symbol() == "f" && cell.fg == tui::style::Color::Magenta));
    assert!(buffer
        .content
        .iter()
        .any(|cell| cell.symbol() == "+" && cell.fg == tui::style::Color::Green));
    assert!(buffer
        .content
        .iter()
        .any(|cell| cell.symbol() == "-" && cell.fg == tui::style::Color::Red));
    assert!(buffer
        .content
        .iter()
        .any(|cell| cell.symbol() == "U" && cell.modifier.contains(tui::style::Modifier::BOLD)));
    assert!(app.close().await.is_empty());
    Ok(())
}

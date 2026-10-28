use editor_config::InlineBlameShow;
use std::{path::Path, process::Command, sync::Arc, time::Duration};
use view::{current, current_ref};

use term::application::Application;
use tokio_stream::wrappers::UnboundedReceiverStream;
use ui_core::input::parse_macro;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

use super::super::helpers::{run_event_loop_until_idle, test_config, AppBuilder};

async fn press_space_b(app: &mut Application) -> anyhow::Result<()> {
    press_keys(app, "<space>B").await
}

async fn repository_status(app: &mut Application, prefix: &str) -> anyhow::Result<()> {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        while !app
            .editor
            .get_status()
            .is_some_and(|(message, _)| message.contains(prefix))
        {
            run_event_loop_until_idle(app).await;
        }
    })
    .await;
    anyhow::ensure!(
        result.is_ok(),
        "Waiting for {prefix}; status: {:?}",
        app.editor.get_status()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn repository_commands_target_the_current_buffer_and_protect_unsaved_edits(
) -> anyhow::Result<()> {
    let (dir, path) = committed_file()?;
    let repo = vcs::operations::GitRepository::discover(&path)?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    press_keys(&mut app, ":vcs-branch-create topic<ret>").await?;
    repository_status(&mut app, "Create branch complete").await?;
    assert!(repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "topic"));
    press_keys(&mut app, "Iunsaved<esc>:vcs-branch-switch topic<ret>").await?;
    repository_status(&mut app, "Save or close unsaved buffer").await?;
    assert!(repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "main" && branch.current));
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "unsavedcommitted\n"
    );
    assert_eq!(std::fs::read_to_string(&path)?, "committed\n");
    press_keys(&mut app, "u:vcs-branch-switch topic<ret>").await?;
    repository_status(&mut app, "Switch branch complete").await?;
    assert!(repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "topic" && branch.current));
    press_keys(&mut app, ":vcs-branches<ret>main").await?;
    wait_for_review_screen(&mut app, "Git branches", "> main").await?;
    press_keys(&mut app, "<ret>").await?;
    repository_status(&mut app, "Switch branch complete").await?;
    assert!(repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "main" && branch.current));
    let cursor = repo.history_cursor(Some(&path))?.unwrap();
    let commit = repo.history_page(cursor).await?.commits.remove(0);
    let endpoint = vcs::operations::history::RevisionFile::new(commit.id, "file.txt".into())?;
    let request = view::features::change_review::HistoricalReviewRequest::revisions(
        &app.editor,
        repo.clone(),
        None,
        Some(endpoint),
    )?;
    app.editor
        .open_historical_review(request, view::editor::Action::Replace);
    assert!(current_ref!(app.editor).1.path().is_none());
    press_keys(&mut app, ":vcs-branch-create history-context<ret>").await?;
    repository_status(&mut app, "Create branch complete").await?;
    assert!(repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "history-context"));
    assert!(app.close().await.is_empty());
    drop(dir);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn repository_worktree_commands_open_keep_buffers_and_confirm_clean_removal(
) -> anyhow::Result<()> {
    let Some(_root) = super::super::helpers::isolation::workspace("test::commands::vcs::repository_worktree_commands_open_keep_buffers_and_confirm_clean_removal")? else { return Ok(()); };
    let (dir, path) = committed_file()?;
    let parent = tempfile::tempdir()?;
    let linked = parent.path().join("linked");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    press_keys(
        &mut app,
        &format!(":vcs-worktree-create {}<ret>", linked.display()),
    )
    .await?;
    repository_status(&mut app, "Create linked worktree complete").await?;
    assert!(linked.join("file.txt").is_file());
    press_keys(
        &mut app,
        &format!("Ikeep<esc>:vcs-worktree-open {}<ret>", linked.display()),
    )
    .await?;
    wait_for_review_screen(&mut app, "Files", "file.txt").await?;
    press_keys(&mut app, "<esc>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "keepcommitted\n"
    );
    assert_eq!(
        stdx::path::canonicalize_existing(&stdx::env::current_working_dir()),
        stdx::path::canonicalize_existing(&linked)
    );
    press_keys(
        &mut app,
        &format!(":vcs-worktree-remove {}<ret>", linked.display()),
    )
    .await?;
    repository_status(&mut app, "Change directory out of this worktree").await?;
    assert!(linked.is_dir());
    press_keys(
        &mut app,
        &format!(
            ":cd {}<ret>:vcs-worktree-remove {}<ret>",
            dir.path().display(),
            linked.display()
        ),
    )
    .await?;
    wait_for_review_screen(&mut app, "Remove linked worktree", "Cancel").await?;
    press_keys(&mut app, "<ret>").await?;
    assert!(linked.is_dir());
    press_keys(
        &mut app,
        &format!(":vcs-worktree-remove {}<ret>", linked.display()),
    )
    .await?;
    wait_for_review_screen(&mut app, "Remove linked worktree", "Cancel").await?;
    press_keys(&mut app, "<down><ret>").await?;
    repository_status(&mut app, "Remove linked worktree complete").await?;
    assert!(!linked.exists());
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "keepcommitted\n"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn repository_remotes_expose_names_and_wait_for_an_explicit_operation() -> anyhow::Result<()>
{
    let (dir, path) = committed_file()?;
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "unavailable-protocol://credential-token@example.invalid/repo",
        ],
    );
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    press_keys(&mut app, ":vcs-remotes<ret>").await?;
    wait_for_review_screen(&mut app, "Git remotes", "origin").await?;
    let screen = review_screen(&app);
    assert!(!screen.contains("credential-token") && !screen.contains("example.invalid"));
    press_keys(&mut app, "<ret>").await?;
    wait_for_review_screen(&mut app, "Remote origin", "Fetch remote").await?;
    assert!(!dir.path().join(".git/FETCH_HEAD").exists());
    assert!(!app.editor.handlers().git_operations.busy());
    press_keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn repository_pull_checks_unsaved_buffers_before_contacting_a_remote() -> anyhow::Result<()> {
    let (dir, path) = committed_file()?;
    let remote = tempfile::tempdir()?;
    git(remote.path(), &["init", "--bare"]);
    git(
        dir.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    press_keys(&mut app, "Iunsaved<esc>:vcs-pull origin<ret>").await?;
    repository_status(&mut app, "Save or close unsaved buffer").await?;
    assert!(!dir.path().join(".git/FETCH_HEAD").exists());
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "unsavedcommitted\n"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

async fn press_keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    for key in parse_macro(keys)? {
        sender.send(Ok(Event::Key(KeyEvent::from(key))))?;
    }
    let mut input = UnboundedReceiverStream::new(receiver);
    assert!(app.event_loop_until_idle(&mut input).await);
    Ok(())
}

fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_1", "main")
        .output()
        .unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn space_b_toggles_inline_blame_and_reuses_the_cached_snapshot() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "committed\n")?;
    std::fs::write(dir.path().join(".gitattributes"), "* text eol=lf\n")?;
    git(dir.path(), &["init"]);
    git(dir.path(), &["add", "file.txt"]);
    git(dir.path(), &["commit", "-m", "initial"]);
    let mut app = AppBuilder::new().with_file(path, None).build()?;
    assert_eq!(
        app.editor.config().inline_blame.show,
        InlineBlameShow::Never
    );
    assert!(current_ref!(app.editor).1.file_blame().is_none());

    press_space_b(&mut app).await?;
    assert_eq!(
        app.editor.config().inline_blame.show,
        InlineBlameShow::CursorLine
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while current_ref!(app.editor).1.file_blame().is_none() {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    let original = current_ref!(app.editor)
        .1
        .file_blame()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(
        current_ref!(app.editor).1.line_blame(0, "{title}").unwrap(),
        "initial"
    );

    press_space_b(&mut app).await?;
    assert_eq!(
        app.editor.config().inline_blame.show,
        InlineBlameShow::Never
    );
    press_space_b(&mut app).await?;
    assert_eq!(
        app.editor.config().inline_blame.show,
        InlineBlameShow::CursorLine
    );
    let current = current_ref!(app.editor)
        .1
        .file_blame()
        .unwrap()
        .as_ref()
        .unwrap();
    assert!(Arc::ptr_eq(&original, current));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn space_b_restores_the_configured_inline_blame_mode() -> anyhow::Result<()> {
    for show in [InlineBlameShow::CursorLine, InlineBlameShow::AllLines] {
        let mut config = test_config();
        config.editor.inline_blame.show = show;
        let mut app = AppBuilder::new().with_config(config).build()?;

        for keys in ["<space>B", "<space>gt", ":vcs-toggle-inline-blame<ret>"] {
            press_keys(&mut app, keys).await?;
            assert_eq!(
                app.editor.config().inline_blame.show,
                InlineBlameShow::Never
            );
            press_keys(&mut app, keys).await?;
            assert_eq!(app.editor.config().inline_blame.show, show);
        }

        // A runtime configuration change replaces the mode restored by the toggle.
        let mut config = (*app.editor.config()).clone();
        config.inline_blame.show = match show {
            InlineBlameShow::CursorLine => InlineBlameShow::AllLines,
            _ => InlineBlameShow::CursorLine,
        };
        let updated_show = config.inline_blame.show;
        app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
        press_space_b(&mut app).await?;
        assert_eq!(
            app.editor.config().inline_blame.show,
            InlineBlameShow::Never
        );
        press_space_b(&mut app).await?;
        assert_eq!(app.editor.config().inline_blame.show, updated_show);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn narrow_views_keep_error_diagnostics_ahead_of_inline_blame() -> anyhow::Result<()> {
    use ::diagnostics::{Diagnostic, DiagnosticProvider, Severity};
    use editor_config::DiagnosticFilter;
    use view::editor::Action;

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "one\n")?;
    std::fs::write(dir.path().join(".gitattributes"), "* text eol=lf\n")?;
    git(dir.path(), &["init"]);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "initial"]);
    let mut config = test_config();
    config.editor.gutters.layout.clear();
    config.editor.inline_blame.show = InlineBlameShow::CursorLine;
    config.editor.inline_blame.format = "BLAME ".repeat(20);
    config.editor.diagnostics.inline.cursor_line = DiagnosticFilter::Disable;
    config.editor.diagnostics.inline.other_lines = DiagnosticFilter::Disable;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(path, None)
        .build()?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while current_ref!(app.editor).1.file_blame().is_none() {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    current!(app.editor).1.replace_diagnostics(
        [Diagnostic {
            range: editor_core::Range::new(0, 1).into(),
            ends_at_word: false,
            starts_at_word: false,
            zero_width: false,
            line: 0,
            message: "error".into(),
            severity: Some(Severity::Error),
            code: None,
            provider: DiagnosticProvider::Spelling,
            tags: vec![],
            source: None,
            data: None,
        }],
        &[],
        None,
    );
    let id = current_ref!(app.editor).1.id();
    for splits in [1, 8] {
        while app.editor.tree.views().count() < splits {
            app.editor.switch(id, Action::VerticalSplit);
        }
        for key in parse_macro("<esc>")? {
            app.handle_terminal_events(Ok(Event::Key(KeyEvent::from(key))))
                .await;
        }
        let (view, doc) = current_ref!(app.editor);
        let area = view.inner_area(doc);
        let buffer = app.terminal_backend().buffer();
        let line: String = (area.x..area.right())
            .map(|x| buffer[(x, area.y)].symbol())
            .collect();
        assert!(line.contains("error"), "{line:?}");
        if splits == 8 {
            assert!(area.width <= 16, "{area:?}");
            assert!(!line.contains("BLAME"), "{line:?}");
        } else {
            assert!(
                line.find("error").unwrap() < line.find("BLAME").unwrap(),
                "{line:?}"
            );
        }
    }
    assert!(app.close().await.is_empty());
    Ok(())
}

#[derive(Default)]
struct TestClipboard(std::sync::Mutex<String>);

impl clipboard::ClipboardBackend for TestClipboard {
    fn name(&self, _: &clipboard::ClipboardProvider) -> String {
        "test".into()
    }
    fn get_contents(
        &self,
        _: &clipboard::ClipboardProvider,
        _: clipboard::ClipboardType,
    ) -> clipboard::Result<String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn set_contents(
        &self,
        _: &clipboard::ClipboardProvider,
        content: &str,
        _: clipboard::ClipboardType,
    ) -> clipboard::Result<()> {
        *self.0.lock().unwrap() = content.into();
        Ok(())
    }
}

fn committed_file() -> anyhow::Result<(tempfile::TempDir, std::path::PathBuf)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "committed\n")?;
    std::fs::write(dir.path().join(".gitattributes"), "* text eol=lf\n")?;
    git(dir.path(), &["init"]);
    git(dir.path(), &["add", "."]);
    git(
        dir.path(),
        &[
            "commit",
            "-m",
            "initial",
            "-m",
            "Full message with **literal** Markdown.\n\nAnother paragraph.",
        ],
    );
    Ok((dir, path))
}

async fn ready_commit_message(app: &mut Application) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let ready = current_ref!(app.editor)
                .1
                .commit_message()
                .is_some_and(|commit| {
                    app.editor
                        .document(commit.review())
                        .and_then(|doc| doc.project_review())
                        .is_some_and(|review| review.is_complete())
                });
            if ready {
                break;
            }
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(app).await;
        }
    })
    .await?;
    Ok(())
}

async fn finish_commit(app: &mut Application) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(app).await;
            if !app.editor.handlers().git_operations.busy()
                && app
                    .editor
                    .get_status()
                    .is_some_and(|(message, _)| !message.starts_with("Preparing commit"))
            {
                break;
            }
        }
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_composer_submits_the_index_and_preserves_unsaved_source_edits() -> anyhow::Result<()>
{
    let (dir, path) = committed_file()?;
    git(dir.path(), &["config", "user.name", "Test Author"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(&path, "staged\n")?;
    git(dir.path(), &["add", "."]);
    std::fs::write(&path, "working tree\n")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let source = current_ref!(app.editor).1.id();
    press_keys(&mut app, "Iunsaved-<esc>:vcs-commit<ret>").await?;
    ready_commit_message(&mut app).await?;
    let composer = current_ref!(app.editor).1.id();
    assert!(current_ref!(app.editor)
        .1
        .display_name()
        .contains("commit message"));
    assert!(!current_ref!(app.editor).1.is_readonly());
    press_keys(&mut app, "iFocused commit<esc>:vcs-commit-submit<ret>").await?;
    finish_commit(&mut app).await?;
    let doc = app.editor.document(composer).unwrap();
    assert!(
        doc.commit_message().unwrap().submitted(),
        "{:?}",
        app.editor.get_status()
    );
    assert!(!doc.is_modified());
    let committed = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["show", "HEAD:file.txt"])
        .output()?;
    assert!(committed.status.success());
    assert_eq!(committed.stdout, b"staged\n");
    assert_eq!(std::fs::read_to_string(&path)?, "working tree\n");
    assert_eq!(
        app.editor.document(source).unwrap().text().to_string(),
        "unsaved-working tree\n"
    );
    assert!(app.editor.close_document(source, true).is_ok());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn failed_commit_keeps_the_message_and_reopening_reuses_the_draft() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let (dir, path) = committed_file()?;
    git(dir.path(), &["config", "user.name", "Test Author"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(&path, "staged\n")?;
    git(dir.path(), &["add", "."]);
    let hook = dir.path().join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho 'hook rejected' >&2\nexit 1\n")?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    press_keys(&mut app, ":vcs-commit<ret>").await?;
    ready_commit_message(&mut app).await?;
    let composer = current_ref!(app.editor).1.id();
    press_keys(&mut app, "iKeep my message<esc>:vcs-commit-submit<ret>").await?;
    finish_commit(&mut app).await?;
    assert!(app.editor.get_status().unwrap().0.contains("hook rejected"));
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "Keep my message\n"
    );
    assert!(current_ref!(app.editor).1.is_modified());
    std::fs::remove_file(hook)?;
    press_keys(&mut app, ":vcs-commit<ret>").await?;
    ready_commit_message(&mut app).await?;
    assert_eq!(current_ref!(app.editor).1.id(), composer);
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "Keep my message\n"
    );
    press_keys(&mut app, ":vcs-commit-submit<ret>").await?;
    finish_commit(&mut app).await?;
    assert!(current_ref!(app.editor)
        .1
        .commit_message()
        .unwrap()
        .submitted());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_composer_from_project_review_keeps_its_repository_and_workspace(
) -> anyhow::Result<()> {
    let (dir, path) = committed_file()?;
    std::fs::write(&path, "staged\n")?;
    git(dir.path(), &["add", "file.txt"]);
    std::fs::write(&path, "unstaged source\n")?;
    let repository = vcs::operations::GitRepository::discover(dir.path())?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let workspace = current_ref!(app.editor).1.workspace_root().to_owned();
    app.editor.open_git_review(
        repository.root().to_owned(),
        workspace.clone(),
        false,
        view::editor::Action::Replace,
    );
    assert!(current_ref!(app.editor)
        .1
        .display_metadata()
        .source_path
        .is_none());
    press_keys(&mut app, "c").await?;
    ready_commit_message(&mut app).await?;
    let composer = current_ref!(app.editor).1.commit_message().unwrap();
    assert_eq!(composer.repository().root(), repository.root());
    assert_eq!(composer.workspace(), workspace);
    let review = app.editor.document(composer.review()).unwrap();
    let state = review.project_review().unwrap();
    assert!(!state.combined);
    assert_eq!(
        state.comparison,
        view::features::change_review::ReviewComparison::Staged
    );
    assert!(state.sections.iter().all(
        |section| section.comparison == view::features::change_review::ReviewComparison::Staged
    ));
    let text = review.text().to_string();
    assert!(text.contains("+staged"), "{text}");
    assert!(!text.contains("unstaged source"), "{text}");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn blame_popup_copies_the_full_id_and_message_from_both_command_surfaces(
) -> anyhow::Result<()> {
    let (_dir, path) = committed_file()?;
    let mut app = AppBuilder::new().with_file(path, None).build()?;
    app.editor
        .registers
        .set_clipboard_backend(Box::<TestClipboard>::default());
    press_keys(&mut app, ":blame<ret>").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !app
            .editor
            .get_status()
            .is_some_and(|(message, _)| message.starts_with("Blame:"))
        {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    let buffer = app.terminal_backend().buffer();
    let screen: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(
        screen.contains("Test Author <test@example.com>"),
        "{screen}"
    );
    assert!(screen.contains("**literal**"), "{screen}");
    assert!(screen.contains("Another paragraph."), "{screen}");
    press_keys(&mut app, "y").await?;
    let id = app.editor.first_register_value('+').unwrap();
    assert_eq!(
        id,
        current_ref!(app.editor)
            .1
            .line_blame_details(0)
            .unwrap()
            .commit_id
    );
    assert_eq!(id.len(), 40);
    press_keys(&mut app, "Y<esc>").await?;
    let message = app.editor.first_register_value('+').unwrap();
    assert_eq!(
        message.trim_end(),
        "initial\n\nFull message with **literal** Markdown.\n\nAnother paragraph."
    );
    press_keys(&mut app, "<space>?blame_line<ret>").await?;
    press_keys(&mut app, "y<esc>").await?;
    assert_eq!(
        app.editor.get_status().unwrap().0.as_ref(),
        "Copied commit ID"
    );
    assert_eq!(
        app.editor.config().inline_blame.show,
        InlineBlameShow::Never
    );
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "committed\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_change_restores_only_the_buffer_and_is_undoable() -> anyhow::Result<()> {
    let (_dir, path) = committed_file()?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    run_event_loop_until_idle(&mut app).await;
    press_keys(&mut app, "Iedited<esc>").await?;
    // Let the asynchronous differ finish before restoring a hunk.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let doc = current_ref!(app.editor).1;
            if doc
                .diff_handle()
                .and_then(|handle| handle.try_load())
                .is_some_and(|diff| diff.is_current() && !diff.is_empty())
            {
                break;
            }
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    press_keys(&mut app, ":restore-change<ret>").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "committed\n");
    press_keys(&mut app, "u").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "editedcommitted\n"
    );
    press_keys(&mut app, "U").await?;
    assert_eq!(std::fs::read_to_string(&path)?, "committed\n");
    git(_dir.path(), &["diff", "--cached", "--exit-code"]);
    assert!(app.close().await.is_empty());
    Ok(())
}

fn partial_index(path: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path.parent().unwrap())
        .args([
            "show",
            &format!(":{}", path.file_name().unwrap().to_str().unwrap()),
        ])
        .env_remove("GIT_DIR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

async fn finish_partial(app: &mut Application) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(app).await;
            if !app.editor.handlers().git_operations.busy() {
                break;
            }
        }
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_staging_commands_include_unsaved_lines_and_preserve_disk_and_other_index_changes(
) -> anyhow::Result<()> {
    let (dir, path) = committed_file()?;
    std::fs::write(&path, "staged\ntwo\nthree\n")?;
    git(dir.path(), &["add", "file.txt"]);
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    run_event_loop_until_idle(&mut app).await;
    press_keys(&mut app, "jIbuffer <esc>:vcs-stage-lines<ret>").await?;
    finish_partial(&mut app).await?;
    assert_eq!(partial_index(&path), "staged\nbuffer two\nthree\n");
    assert_eq!(std::fs::read_to_string(&path)?, "staged\ntwo\nthree\n");
    assert!(current_ref!(app.editor).1.is_modified());
    press_keys(&mut app, ":vcs-unstage-lines<ret>").await?;
    finish_partial(&mut app).await?;
    assert_eq!(partial_index(&path), "staged\nthree\n");
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .contains("buffer two"));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_staging_and_next_navigates_the_remaining_index_diff() -> anyhow::Result<()> {
    let (dir, path) = committed_file()?;
    std::fs::write(&path, "first\nkeep\nlast\n")?;
    git(dir.path(), &["add", "file.txt"]);
    git(dir.path(), &["commit", "-m", "multiple lines"]);
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    run_event_loop_until_idle(&mut app).await;
    press_keys(&mut app, "Inew <esc>jjInew <esc>gg:vcs-stage-and-next<ret>").await?;
    finish_partial(&mut app).await?;
    assert_eq!(partial_index(&path), "new first\nkeep\nlast\n");
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.cursor_line(view.id), 2);
    assert_eq!(std::fs::read_to_string(&path)?, "first\nkeep\nlast\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_staging_live_review_can_stage_only_a_removed_row() -> anyhow::Result<()> {
    use editor_core::Selection;
    use stdx::rope::RopeExt;
    let (_dir, path) = committed_file()?;
    std::fs::write(&path, "replacement\n")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    run_event_loop_until_idle(&mut app).await;
    press_keys(&mut app, ":diff buffer<ret>").await?;
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
    {
        let (view, doc) = current!(app.editor);
        let review = doc.review().unwrap();
        let row = review
            .review()
            .lines
            .iter()
            .position(|row| row.kind == view::features::change_review::ReviewLineKind::Removed)
            .unwrap()
            + review.row_offset();
        let point = doc.text().line_to_char_idx(row);
        doc.set_selection(view.id, Selection::point(point));
    }
    press_keys(&mut app, ":vcs-stage-lines<ret>").await?;
    finish_partial(&mut app).await?;
    assert_eq!(partial_index(&path), "");
    assert_eq!(std::fs::read_to_string(&path)?, "replacement\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_file_pickers_include_staged_only_files() -> anyhow::Result<()> {
    let Some(root) = super::super::helpers::isolation::workspace(
        "test::commands::vcs::changed_file_pickers_include_staged_only_files",
    )?
    else {
        return Ok(());
    };
    let repo = root.join("workspace");
    let existing = repo.join("existing.txt");
    std::fs::write(&existing, "before\n")?;
    std::fs::write(repo.join(".gitattributes"), "* text eol=lf\n")?;
    git(&repo, &["init"]);
    git(&repo, &["add", "existing.txt", ".gitattributes"]);
    git(&repo, &["commit", "-m", "initial"]);
    std::fs::write(&existing, "staged modification\n")?;
    std::fs::write(repo.join("staged-only.txt"), "staged addition\n")?;
    git(&repo, &["add", "existing.txt", "staged-only.txt"]);
    std::fs::write(repo.join("untracked.txt"), "untracked\n")?;

    let mut config = test_config();
    config.editor.icons = false;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&existing, None)
        .build()?;
    run_event_loop_until_idle(&mut app).await;
    for keys in [
        "<space>gs",
        ":vcs-changes<ret>",
        ":vcs-repository-changes<ret>",
        "<space>?vcs-changes<ret>",
        "<space>?changed_file_picker_in_repository<ret>",
    ] {
        press_keys(&mut app, keys).await?;
        let screen = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let screen: String = app
                    .terminal_backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                if !screen.contains("(running)") {
                    break screen;
                }
                run_event_loop_until_idle(&mut app).await;
            }
        })
        .await?;
        for text in [
            "existing.txt",
            "staged-only.txt",
            "untracked.txt",
            "+ added",
            "~ modified",
            "+ untracked",
        ] {
            assert!(screen.contains(text), "missing {text}: {screen}");
        }
        press_keys(&mut app, "<esc>").await?;
    }
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn change_navigation_respects_wrap_policy_and_counts() -> anyhow::Result<()> {
    use editor_core::Selection;
    use stdx::rope::RopeExt;
    for wrap in [false, true] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("changes.txt");
        // Keep Git's diff baseline in LF even when core.autocrlf is enabled.
        std::fs::write(dir.path().join(".gitattributes"), "* text eol=lf\n")?;
        std::fs::write(&path, "one\ntwo\nthree\nfour\nfive\nsix\nseven\n")?;
        git(dir.path(), &["init"]);
        git(dir.path(), &["add", "changes.txt"]);
        git(dir.path(), &["commit", "-m", "initial"]);
        std::fs::write(&path, "one\nTWO\nthree\nfour\nFIVE\nsix\nseven\n")?;
        let mut config = test_config();
        config.editor.search.wrap_around = wrap;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_file(&path, None)
            .build()?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if current_ref!(app.editor)
                    .1
                    .diff_handle()
                    .and_then(|handle| handle.try_load())
                    .is_some_and(|diff| diff.is_current() && diff.len() == 2)
                {
                    break;
                }
                run_event_loop_until_idle(&mut app).await;
            }
        })
        .await?;
        for (keys, start, expected) in [
            ("]g", 6, if wrap { 1 } else { 6 }),
            ("[g", 0, if wrap { 4 } else { 0 }),
            ("2]g", 6, if wrap { 4 } else { 6 }),
            ("2[g", 0, if wrap { 1 } else { 0 }),
        ] {
            let (view, doc) = current!(app.editor);
            let position = doc.text().line_to_char_idx(start);
            doc.set_selection(view.id, Selection::point(position));
            press_keys(&mut app, keys).await?;
            let (view, doc) = current_ref!(app.editor);
            assert_eq!(
                doc.selection(view.id)
                    .primary()
                    .cursor_line(doc.text().slice(..)),
                expected,
                "{keys}, wrap={wrap}"
            );
            assert_eq!(
                app.editor.get_status().unwrap().0.as_ref(),
                if wrap {
                    "Wrapped around document"
                } else {
                    "No more changes in document"
                }
            );
        }
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn show_change_displays_deleted_and_unsaved_text_without_writing() -> anyhow::Result<()> {
    let (dir, path) = committed_file()?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    press_keys(&mut app, "Iunsaved-<esc>").await?;
    for keys in [
        ":show-change<ret>",
        ":vcs-show-change<ret>",
        "<space>i",
        "<space>gi",
    ] {
        press_keys(&mut app, keys).await?;
        wait_for_review_screen(&mut app, "Current change", "− committed").await?;
        let screen = review_screen(&app);
        assert!(screen.contains("+ unsaved-committed"));
        assert!(screen.contains("+1 −1"));
        let (view, doc) = current_ref!(app.editor);
        let selection = doc.selection(view.id).clone();
        press_keys(&mut app, "<right><left>").await?;
        let (view, doc) = current_ref!(app.editor);
        assert_eq!(doc.selection(view.id), &selection);
        assert!(review_screen(&app).contains("Current change"));
        press_keys(&mut app, "<esc>").await?;
    }
    assert_eq!(std::fs::read_to_string(path)?, "committed\n");
    git(dir.path(), &["diff", "--cached", "--exit-code"]);
    // Drop the unsaved edit before normal shutdown.
    press_keys(&mut app, "u").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn change_popup_restores_with_default_and_remapped_shortcuts() -> anyhow::Result<()> {
    for shortcut in ["r", "<A-r>"] {
        let (dir, path) = committed_file()?;
        let mut config = test_config();
        if shortcut == "<A-r>" {
            config.component_keys = term::config::Config::load(
                Ok(&"[keys.diff]\nr = 'unbound'\nA-r = 'restore'\n".to_owned()),
                Err(term::config::ConfigLoadError::default()),
            )
            .unwrap()
            .component_keys;
        }
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_file(&path, None)
            .build()?;
        press_keys(&mut app, "Iunsaved-<esc><space>i").await?;
        wait_for_review_screen(&mut app, "Current change", "restore hunk from HEAD").await?;
        if shortcut == "<A-r>" {
            assert!(review_screen(&app).contains("<A-r> restore"));
        }
        press_keys(&mut app, shortcut).await?;
        assert!(!review_screen(&app).contains("Current change"));
        assert_eq!(current_ref!(app.editor).1.text().to_string(), "committed\n");
        press_keys(&mut app, "u").await?;
        assert_eq!(
            current_ref!(app.editor).1.text().to_string(),
            "unsaved-committed\n"
        );
        assert_eq!(std::fs::read_to_string(&path)?, "committed\n");
        git(dir.path(), &["diff", "--cached", "--exit-code"]);
        press_keys(&mut app, "u").await?;
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[cfg(feature = "syntax-diff")]
#[tokio::test(flavor = "multi_thread")]
async fn syntax_hunk_reviews_follow_configuration_and_keep_captured_snapshots() -> anyhow::Result<()>
{
    use editor_config::DiffEngine;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.json");
    std::fs::write(&path, "{\"value\": 1}\n")?;
    std::fs::write(dir.path().join(".gitattributes"), "* text eol=lf\n")?;
    git(dir.path(), &["init"]);
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-m", "initial"]);
    std::fs::write(&path, "{\n  \"value\": 2\n}\n")?;
    let index = std::fs::read(dir.path().join(".git/index"))?;
    let mut config = test_config();
    config.editor.diff.engine = DiffEngine::Syntax;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&path, None)
        .build()?;
    press_keys(&mut app, ":show-change<ret>").await?;
    wait_for_review_screen(&mut app, "syntax · Current change", "\"value\": 2").await?;
    let (view, doc) = current_ref!(app.editor);
    let selection = doc.selection(view.id).clone();
    let version = doc.version();
    // Scrolling an open popup keeps the configured engine and captured text.
    std::fs::write(&path, "{\"value\": 999}\n")?;
    press_keys(&mut app, "<right><left>").await?;
    wait_for_review_screen(&mut app, "syntax · Current change", "\"value\": 2").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.selection(view.id), &selection);
    assert_eq!(doc.version(), version);
    assert_eq!(std::fs::read(dir.path().join(".git/index"))?, index);
    press_keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[cfg(feature = "syntax-diff")]
#[tokio::test(flavor = "multi_thread")]
async fn syntax_picker_reviews_toggle_comparisons_open_buffers_and_report_fallbacks(
) -> anyhow::Result<()> {
    let Some(root) = super::super::helpers::isolation::workspace(
        "test::commands::vcs::syntax_picker_reviews_toggle_comparisons_open_buffers_and_report_fallbacks",
    )? else { return Ok(()); };
    let repo = root.join("workspace");
    let path = repo.join("file.json");
    std::fs::write(&path, "{\"value\": 1}\n")?;
    std::fs::write(repo.join(".gitattributes"), "* text eol=lf\n")?;
    git(&repo, &["init"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "initial"]);
    std::fs::write(&path, "{\"value\": 2}\n")?;
    git(&repo, &["add", "file.json"]);
    std::fs::write(&path, "{\"value\": 3}\n")?;
    let index = std::fs::read(repo.join(".git/index"))?;
    let mut config = test_config();
    config.editor.diff.engine = editor_config::DiffEngine::Syntax;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&path, None)
        .build()?;
    press_keys(&mut app, "<space>gsfile.json").await?;
    wait_for_review_screen(&mut app, "syntax · Unstaged", "\"value\": 3").await?;
    press_keys(&mut app, "<A-d>").await?;
    wait_for_review_screen(&mut app, "syntax · Unstaged", "\"value\": 3").await?;
    // Opening a buffer uses the configured engine and keeps the picker for restoration.
    press_keys(&mut app, "<A-o>").await?;
    wait_for_review_buffer(&mut app, "syntax", "\"value\": 3").await?;
    press_keys(&mut app, "<space>'").await?;
    wait_for_review_screen(&mut app, "syntax · Unstaged", "\"value\": 3").await?;
    press_keys(&mut app, "<A-left>").await?;
    wait_for_review_screen(&mut app, "syntax · Staged", "\"value\": 1").await?;
    press_keys(&mut app, "<A-o>").await?;
    wait_for_review_buffer(&mut app, "syntax", "\"value\": 2").await?;
    // A hidden preview also opens a review using the configured engine.
    press_keys(&mut app, "<space>'<C-t><A-o>").await?;
    wait_for_review_buffer(&mut app, "syntax", "\"value\": 2").await?;
    press_keys(&mut app, "<space>'<C-t><esc>").await?;
    assert_eq!(std::fs::read(repo.join(".git/index"))?, index);
    std::fs::write(&path, "{]\n")?;
    press_keys(&mut app, "<space>gsfile.json").await?;
    wait_for_review_screen(&mut app, "line · Unstaged", "parse errors").await?;
    assert!(review_screen(&app).contains("{]"));
    press_keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_file_review_switches_layers_opens_deletions_and_preserves_the_index(
) -> anyhow::Result<()> {
    let Some(root) = super::super::helpers::isolation::workspace(
        "test::commands::vcs::changed_file_review_switches_layers_opens_deletions_and_preserves_the_index",
    )? else { return Ok(()); };
    let repo = root.join("workspace");
    let path = repo.join("file.txt");
    std::fs::write(&path, "head contents\n")?;
    std::fs::write(repo.join(".gitattributes"), "* text eol=lf\n")?;
    git(&repo, &["init"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "initial"]);
    std::fs::write(&path, "staged contents\n")?;
    git(&repo, &["add", "file.txt"]);
    std::fs::write(&path, "working contents\n")?;
    let index = std::fs::read(repo.join(".git/index"))?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    press_keys(&mut app, "Iunsaved-<esc><space>gsfile.txt").await?;
    wait_for_review_screen(&mut app, "Unsaved · index", "unsaved-working contents").await?;
    assert!(review_screen(&app).contains("A-p/A-n versions"));
    press_keys(&mut app, "<A-n>").await?;
    wait_for_review_screen(&mut app, "Staged · HEAD", "head contents").await?;
    press_keys(&mut app, "<A-p>").await?;
    wait_for_review_screen(&mut app, "Unsaved · index", "unsaved-working contents").await?;
    press_keys(&mut app, "<A-right>").await?;
    wait_for_review_screen(&mut app, "Staged · HEAD", "head contents").await?;
    let screen = review_screen(&app);
    assert!(screen.contains("staged contents"));
    assert!(!screen.contains("working contents"));
    press_keys(&mut app, "<A-n>").await?;
    wait_for_review_screen(&mut app, "Unstaged · index", "working contents").await?;
    press_keys(&mut app, "<A-left>").await?;
    wait_for_review_screen(&mut app, "Staged · HEAD", "head contents").await?;
    press_keys(&mut app, "<A-right>").await?;
    wait_for_review_screen(&mut app, "Unstaged · index", "working contents").await?;
    assert!(review_screen(&app).contains("staged contents"));
    press_keys(&mut app, "<A-o>").await?;
    wait_for_review_buffer(&mut app, "line", "working contents").await?;
    press_keys(&mut app, "<C-o><space>'<C-t><A-o>").await?;
    wait_for_review_buffer(&mut app, "line", "working contents").await?;
    press_keys(&mut app, "<C-o><space>'<C-t><esc>").await?;
    assert_eq!(std::fs::read(repo.join(".git/index"))?, index);
    assert_eq!(std::fs::read_to_string(&path)?, "working contents\n");
    // Opening a deleted row creates a pathless review and never recreates the file.
    press_keys(&mut app, "u").await?;
    std::fs::remove_file(&path)?;
    press_keys(&mut app, "<space>gsfile.txt").await?;
    wait_for_review_screen(&mut app, "Unstaged · index", "staged contents").await?;
    let documents = app.editor.documents().count();
    press_keys(&mut app, "<ret>").await?;
    wait_for_review_buffer(&mut app, "line", "staged contents").await?;
    assert_eq!(app.editor.documents().count(), documents + 1);
    assert!(current_ref!(app.editor).1.path().is_none());
    assert!(!path.exists());
    assert!(app.close().await.is_empty());
    Ok(())
}

fn review_screen(app: &Application) -> String {
    app.terminal_backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

async fn wait_for_review_buffer(
    app: &mut Application,
    engine: &str,
    contents: &str,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, doc) = current_ref!(app.editor);
            if doc.review().is_some_and(|review| {
                !review.is_loading() && review.review().provider.unwrap_or("line") == engine
            }) && doc.text().to_string().contains(contents)
            {
                break;
            }
            run_event_loop_until_idle(app).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("Missing review buffer using {engine:?} with {contents:?}"))
}

async fn wait_for_review_screen(
    app: &mut Application,
    heading: &str,
    contents: &str,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let screen = review_screen(app);
            if screen.contains(heading)
                && screen.contains(contents)
                && !screen.contains("Loading diff")
                && !screen.contains("(running)")
            {
                break;
            }
            run_event_loop_until_idle(app).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("Missing {heading:?} / {contents:?}: {}", review_screen(app)))
}

#[tokio::test(flavor = "multi_thread")]
async fn review_refreshes_after_external_staging_and_edits_without_polling() -> anyhow::Result<()> {
    let Some(root) = super::super::helpers::isolation::workspace(
        "test::commands::vcs::review_refreshes_after_external_staging_and_edits_without_polling",
    )?
    else {
        return Ok(());
    };
    let repo = root.join("workspace");
    let path = repo.join("watched.txt");
    std::fs::write(&path, "before\n")?;
    std::fs::write(repo.join(".gitattributes"), "* text eol=lf\n")?;
    git(&repo, &["init"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "initial"]);
    std::fs::write(&path, "first edit\n")?;
    let mut config = test_config();
    config.editor.files.watch.enable = true;
    config.editor.files.watch.watch_vcs = true;
    config.editor.files.reload.poll.enable = false;
    config.editor.files.reload.enable = false;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&path, None)
        .build()?;
    press_keys(&mut app, "<space>gswatched.txt").await?;
    wait_for_review_screen(&mut app, "Unstaged · index", "first edit").await?;
    git(&repo, &["add", "watched.txt"]);
    wait_for_review_screen(&mut app, "Staged · HEAD", "first edit").await?;
    std::fs::write(&path, "second edit\n")?;
    wait_for_review_screen(&mut app, "Unstaged · index", "second edit").await?;
    assert!(review_screen(&app).contains("first edit"));
    assert!(review_screen(&app).contains("1/1"));
    press_keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_resolution_is_undoable_and_never_writes_disk_or_index() -> anyhow::Result<()> {
    let (dir, path) = committed_file()?;
    let original = "prefix\n<<<<<<< HEAD\ncurrent\n||||||| base\ncommitted\n=======\nincoming\n>>>>>>> topic\nsuffix\n";
    std::fs::write(&path, original)?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    run_event_loop_until_idle(&mut app).await;
    press_keys(&mut app, "]m").await?;
    press_keys(&mut app, ":vcs-conflict-resolve both<ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "prefix\ncurrent\nincoming\nsuffix\n"
    );
    assert_eq!(std::fs::read_to_string(&path)?, original);
    git(dir.path(), &["diff", "--cached", "--exit-code"]);
    press_keys(&mut app, "u").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), original);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_navigation_wraps_and_resolution_leaves_other_conflicts_untouched(
) -> anyhow::Result<()> {
    let first = "<<<<<<< HEAD\none\n=======\ntwo\n>>>>>>> topic\n";
    let second = "<<<<<<< HEAD\nthree\n=======\nfour\n>>>>>>> topic\n";
    let text = format!("prefix\n{first}middle\n{second}");
    let mut app = AppBuilder::new()
        .with_input_text(format!("#[|]#{text}"))
        .build()?;
    run_event_loop_until_idle(&mut app).await;
    press_keys(&mut app, "]m]m").await?;
    press_keys(&mut app, ":vcs-conflict-resolve incoming<ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        format!("prefix\n{first}middle\nfour\n")
    );
    press_keys(&mut app, "]m").await?;
    press_keys(&mut app, ":vcs-conflict-resolve current<ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "prefix\none\nmiddle\nfour\n"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

async fn requested_actions(app: &mut Application) -> anyhow::Result<Vec<view::action::Action>> {
    let update = app
        .editor
        .request_code_actions()
        .run()
        .await
        .ok_or_else(|| anyhow::anyhow!("action request canceled"))?;
    update
        .into_actions(&app.editor)
        .ok_or_else(|| anyhow::anyhow!("action request stale"))
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_actions_resolve_each_choice_without_a_language_server() -> anyhow::Result<()> {
    let original = "préfix\r\n<<<<<<< HEAD\r\nours\r\n||||||| base\r\nold\r\n=======\r\ntheirs\r\n>>>>>>> topic\r\nsuffix\r\n";
    for (title, replacement) in [
        ("Accept current", "ours\r\n"),
        ("Accept incoming", "theirs\r\n"),
        ("Accept both (current then incoming)", "ours\r\ntheirs\r\n"),
    ] {
        let mut config = test_config();
        config.editor.lsp.enable = false;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_input_text(format!("#[|]#{original}"))
            .build()?;
        press_keys(&mut app, "]m").await?;
        let actions = requested_actions(&mut app).await?;
        assert_eq!(actions.len(), 3);
        actions
            .iter()
            .find(|action| action.title() == title)
            .unwrap()
            .execute(&mut app.editor);
        assert_eq!(
            current_ref!(app.editor).1.text().to_string(),
            format!("préfix\r\n{replacement}suffix\r\n")
        );
        press_keys(&mut app, "u").await?;
        assert_eq!(current_ref!(app.editor).1.text().to_string(), original);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_actions_reject_stale_selection_and_document_snapshots() -> anyhow::Result<()> {
    let original = "prefix\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\nsuffix\n";
    let mut app = AppBuilder::new()
        .with_input_text(format!("#[|]#{original}"))
        .build()?;
    assert!(requested_actions(&mut app).await?.is_empty());
    press_keys(&mut app, "]m").await?;
    let actions = requested_actions(&mut app).await?;
    press_keys(&mut app, "j").await?;
    actions[0].execute(&mut app.editor);
    assert_eq!(current_ref!(app.editor).1.text().to_string(), original);
    let actions = requested_actions(&mut app).await?;
    press_keys(&mut app, "iX<esc>").await?;
    let edited = current_ref!(app.editor).1.text().to_string();
    actions[0].execute(&mut app.editor);
    assert_eq!(current_ref!(app.editor).1.text().to_string(), edited);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn space_a_offers_conflict_resolution_and_preserves_disk_and_index() -> anyhow::Result<()> {
    let (dir, path) = committed_file()?;
    let original = "prefix\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\n";
    std::fs::write(&path, original)?;
    let index = std::fs::read(dir.path().join(".git/index"))?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    press_keys(&mut app, "]m<space>a").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (tx, rx) = tokio::sync::oneshot::channel();
            app.job_sender()
                .send(move |_, compositor| {
                    let selected = compositor
                        .find_id::<term::ui::Popup<term::ui::Menu<view::action::Action>>>(
                            "code-action",
                        )
                        .and_then(|popup| popup.contents().selection())
                        .map(|action| action.title().to_owned());
                    let _ = tx.send(selected);
                })
                .await;
            run_event_loop_until_idle(&mut app).await;
            if let Some(title) = rx.await? {
                assert_eq!(title, "Accept current");
                break;
            }
        }
        anyhow::Ok(())
    })
    .await??;
    press_keys(&mut app, "<C-n><ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "prefix\ntheirs\n"
    );
    assert_eq!(std::fs::read_to_string(&path)?, original);
    assert_eq!(std::fs::read(dir.path().join(".git/index"))?, index);
    press_keys(&mut app, "u").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), original);
    assert!(app.close().await.is_empty());
    Ok(())
}

fn create_index_conflict(repo: &Path) {
    create_encoded_index_conflict(repo, |text| text.as_bytes().to_vec());
    assert!(std::fs::read_to_string(repo.join("file.txt"))
        .unwrap()
        .contains("<<<<<<<"));
}

fn create_encoded_index_conflict(repo: &Path, encode: impl Fn(&str) -> Vec<u8>) {
    let path = repo.join("file.txt");
    std::fs::write(&path, encode("base\n")).unwrap();
    std::fs::write(repo.join(".gitattributes"), "* text eol=lf\n").unwrap();
    git(repo, &["init"]);
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-m", "base"]);
    git(repo, &["checkout", "-b", "incoming"]);
    std::fs::write(&path, encode("theirs\n")).unwrap();
    git(repo, &["commit", "-am", "incoming"]);
    git(repo, &["checkout", "main"]);
    std::fs::write(&path, encode("ours\n")).unwrap();
    git(repo, &["commit", "-am", "current"]);
    let merge = git_output(repo, &["merge", "incoming"]);
    assert_eq!(
        merge.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn conflicted_file_preview_shows_sections_and_tracks_unsaved_resolution() -> anyhow::Result<()>
{
    let Some(root) = super::super::helpers::isolation::workspace(
        "test::commands::vcs::conflicted_file_preview_shows_sections_and_tracks_unsaved_resolution",
    )?
    else {
        return Ok(());
    };
    let repo = root.join("workspace");
    let path = repo.join("file.txt");
    create_index_conflict(&repo);
    let original = std::fs::read_to_string(&path)?;
    let index = std::fs::read(repo.join(".git/index"))?;
    let mut app = AppBuilder::new().build()?;
    press_keys(&mut app, "<space>gsfile.txt").await?;
    wait_for_review_screen(&mut app, "Conflicts · file.txt", "Current: HEAD").await?;
    assert!(!review_screen(&app).contains("<<<<<<<"));
    press_keys(&mut app, "<A-n><A-p>").await?;
    wait_for_review_screen(&mut app, "Conflicts · file.txt", "theirs").await?;
    assert!(!review_screen(&app).contains("Unmerged file"));
    assert!(!review_screen(&app).contains("versions"));
    press_keys(&mut app, "<ret>").await?;
    assert_eq!(current_ref!(app.editor).1.path(), Some(path.as_path()));
    assert!(current_ref!(app.editor).1.review().is_none());
    press_keys(
        &mut app,
        "]m:vcs-conflict-resolve incoming<ret><space>gsfile.txt",
    )
    .await?;
    wait_for_review_screen(&mut app, "Changes · HEAD → buffer", "theirs").await?;
    assert!(!review_screen(&app).contains("<<<<<<<"));
    press_keys(&mut app, "<A-n><A-p>").await?;
    wait_for_review_screen(&mut app, "Changes · HEAD → buffer", "theirs").await?;
    press_keys(&mut app, "<esc>").await?;
    assert_eq!(std::fs::read_to_string(&path)?, original);
    assert_eq!(std::fs::read(repo.join(".git/index"))?, index);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_actions_ignore_malformed_and_immutable_documents() -> anyhow::Result<()> {
    let malformed = "<<<<<<< HEAD\nours\n=======\ntheirs\n";
    let mut app = AppBuilder::new()
        .with_input_text(format!("#[|]#{malformed}"))
        .build()?;
    assert!(requested_actions(&mut app).await?.is_empty());
    let original = "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\n";
    view::features::conflicts::open_source(
        &mut app.editor,
        "Read-only source".into(),
        editor_core::Rope::from(original),
    );
    assert!(current_ref!(app.editor).1.is_readonly());
    assert!(requested_actions(&mut app).await?.is_empty());
    assert!(view::features::conflicts::resolve(
        &mut app.editor,
        vcs::conflict::Resolution::Current
    )
    .is_err());
    assert_eq!(current_ref!(app.editor).1.text().to_string(), original);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_source_opens_the_index_stage_in_a_readonly_split() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    create_index_conflict(dir.path());
    let path = dir.path().join("file.txt");
    let index = std::fs::read(dir.path().join(".git/index"))?;
    let original = std::fs::read_to_string(&path)?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    press_keys(&mut app, ":vcs-conflict-source base<ret>").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !current_ref!(app.editor).1.is_readonly() {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    let (_, doc) = current_ref!(app.editor);
    assert!(doc.path().is_none());
    assert_eq!(doc.text().to_string(), "base\n");
    assert!(requested_actions(&mut app).await?.is_empty());
    assert_eq!(std::fs::read_to_string(&path)?, original);
    assert_eq!(std::fs::read(dir.path().join(".git/index"))?, index);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_headings_stay_stable_during_navigation_and_follow_undo() -> anyhow::Result<()> {
    let original =
        "préfix\n<<<<<<< HEAD\nours\n||||||| base\nold\n=======\ntheirs\n>>>>>>> topic\nsuffix\n";
    let (dir, path) = committed_file()?;
    std::fs::write(&path, original)?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    run_event_loop_until_idle(&mut app).await;
    let screen = review_screen(&app);
    assert!(screen.contains("Current: HEAD"), "{screen}");
    assert!(screen.contains("Incoming: topic"));
    assert!(screen.contains("Base"));
    assert!(!screen.contains("<<<<<<<"));
    assert!(!screen.contains("======="));
    assert!(screen.contains("│"));
    assert!(!screen.contains("▍"));
    assert_eq!(current_ref!(app.editor).1.text().to_string(), original);

    // Navigating through labels must not reveal raw markers or lose the caret.
    press_keys(&mut app, "]mlll").await?;
    assert!(!review_screen(&app).contains("<<<<<<< HEAD"));
    let (view, doc) = current_ref!(app.editor);
    assert!(app.editor.cursor_cache.get(view, doc).is_some());
    press_keys(&mut app, "6j").await?;
    assert!(review_screen(&app).contains("End conflict"));
    assert!(!review_screen(&app).contains(">>>>>>> topic"));
    let (view, doc) = current_ref!(app.editor);
    assert!(app.editor.cursor_cache.get(view, doc).is_some());
    press_keys(&mut app, "]mj").await?;
    press_keys(&mut app, ":vcs-conflict-toggle-markers<ret>").await?;
    assert!(review_screen(&app).contains("<<<<<<< HEAD"));
    assert!(review_screen(&app).contains("======="));
    assert!(current_ref!(app.editor)
        .1
        .raw_conflict_markers(current_ref!(app.editor).0.id));
    let raw_view = current_ref!(app.editor).0.id;
    press_keys(&mut app, "<C-w>v").await?;
    let split_view = current_ref!(app.editor).0.id;
    assert_ne!(raw_view, split_view);
    assert!(!current_ref!(app.editor).1.raw_conflict_markers(split_view));
    assert!(current_ref!(app.editor).1.raw_conflict_markers(raw_view));
    press_keys(&mut app, ":q<ret>").await?;
    press_keys(&mut app, ":vcs-conflict-toggle-markers<ret>").await?;
    assert!(!review_screen(&app).contains("<<<<<<<"));
    press_keys(&mut app, ":vcs-conflict-resolve incoming<ret>").await?;
    assert!(!current_ref!(app.editor).1.conflict_presentation().active());
    assert!(!review_screen(&app).contains("Current: HEAD"));
    press_keys(&mut app, "u").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), original);
    assert!(current_ref!(app.editor).1.conflict_presentation().active());
    press_keys(&mut app, "gg").await?;
    assert!(review_screen(&app).contains("Current: HEAD"));

    // Breaking a marker invalidates every heading; undo rebuilds the model.
    press_keys(&mut app, "]miX<esc>").await?;
    assert!(current_ref!(app.editor)
        .1
        .conflict_presentation()
        .error
        .is_some());
    assert!(current_ref!(app.editor)
        .1
        .conflict_presentation()
        .markers
        .is_empty());
    assert!(requested_actions(&mut app).await?.is_empty());
    press_keys(&mut app, "ugg").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), original);
    assert!(review_screen(&app).contains("Current: HEAD"));
    assert_eq!(std::fs::read_to_string(&path)?, original);
    git(dir.path(), &["diff", "--cached", "--exit-code"]);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_source_decodes_utf16_index_stages() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    create_encoded_index_conflict(dir.path(), |text| {
        [0xff, 0xfe]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect()
    });
    let path = dir.path().join("file.txt");
    let before = std::fs::read(&path)?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    assert_eq!(
        current_ref!(app.editor).1.encoding(),
        editor_core::encoding::UTF_16LE
    );
    press_keys(&mut app, ":vcs-conflict-source base<ret>").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !current_ref!(app.editor).1.is_readonly() {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "base\n");
    assert_eq!(std::fs::read(&path)?, before);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_navigation_counts_wrap_and_resolution_macros_replay() -> anyhow::Result<()> {
    let block = "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> incoming\n";
    let original = format!("prefix\n{block}middle\n{block}suffix\n{block}");
    let mut app = AppBuilder::new()
        .with_input_text(format!("#[|]#{original}"))
        .build()?;
    press_keys(&mut app, "2]m").await?;
    let (view, doc) = current_ref!(app.editor);
    let second = original.match_indices("<<<<<<<").nth(1).unwrap().0;
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        second
    );
    press_keys(&mut app, "3]m").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        second
    );
    press_keys(&mut app, "2[m").await?;
    // Record navigation and resolution, then replay against the next region.
    press_keys(&mut app, "Q]m:vcs-conflict-resolve incoming<ret>Qq").await?;
    assert_eq!(
        current_ref!(app.editor)
            .1
            .conflict_presentation()
            .regions
            .len(),
        1
    );
    press_keys(&mut app, "uu").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), original);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_staging_rejects_a_buffer_changed_after_preparation() -> anyhow::Result<()> {
    let (_dir, path) = committed_file()?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    run_event_loop_until_idle(&mut app).await;
    press_keys(&mut app, "Ifirst <esc>").await?;
    let (request, _) = view::features::git_operations::partial::capture(
        &app.editor,
        vcs::operations::FileOperation::Stage,
        false,
        false,
    )?;
    let prepared = request.prepare().await?;
    press_keys(&mut app, "Inewer <esc>").await?;
    assert!(prepared.admit(&mut app.editor).is_err());
    assert_eq!(partial_index(&path), "committed\n");
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .contains("newer"));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_staging_popup_actions_use_index_comparisons_and_follow_remaps(
) -> anyhow::Result<()> {
    for (stage, unstage) in [("s", "u"), ("<A-s>", "<A-u>")] {
        let (dir, path) = committed_file()?;
        std::fs::write(&path, "staged\n")?;
        git(dir.path(), &["add", "file.txt"]);
        let mut config = test_config();
        if stage == "<A-s>" {
            config.component_keys = term::config::Config::load(
                Ok(
                    &"[keys.diff]\ns = 'unbound'\nu = 'unbound'\nA-s = 'stage'\nA-u = 'unstage'\n"
                        .to_owned(),
                ),
                Err(term::config::ConfigLoadError::default()),
            )
            .unwrap()
            .component_keys;
        }
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_file(&path, None)
            .build()?;
        press_keys(&mut app, "Iunsaved-<esc><space>i").await?;
        wait_for_review_screen(&mut app, "Current change", "stage: index").await?;
        assert!(review_screen(&app).contains("unstage: HEAD"));
        if stage == "<A-s>" {
            assert!(review_screen(&app).contains("<A-s> stage"));
        }
        press_keys(&mut app, stage).await?;
        finish_partial(&mut app).await?;
        assert_eq!(partial_index(&path), "unsaved-staged\n");
        assert!(!review_screen(&app).contains("Current change"));
        press_keys(&mut app, "<space>i").await?;
        wait_for_review_screen(&mut app, "Current change", "unstage: HEAD").await?;
        press_keys(&mut app, unstage).await?;
        finish_partial(&mut app).await?;
        assert_eq!(partial_index(&path), "committed\n");
        assert_eq!(std::fs::read_to_string(&path)?, "staged\n");
        assert_eq!(
            current_ref!(app.editor).1.text().to_string(),
            "unsaved-staged\n"
        );
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_staging_popup_rejects_a_changed_repository_snapshot() -> anyhow::Result<()> {
    let (dir, path) = committed_file()?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    press_keys(&mut app, "Iunsaved-<esc><space>i").await?;
    wait_for_review_screen(&mut app, "Current change", "stage: index").await?;
    std::fs::write(&path, "external\n")?;
    git(dir.path(), &["add", "file.txt"]);
    press_keys(&mut app, "s").await?;
    finish_partial(&mut app).await?;
    assert_eq!(partial_index(&path), "external\n");
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "unsaved-committed\n"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_staging_popup_and_next_affects_only_the_cursor_hunk() -> anyhow::Result<()> {
    use editor_core::{Range, Selection};
    use stdx::rope::RopeExt;
    let (dir, path) = committed_file()?;
    std::fs::write(&path, "first\nkeep\nlast\n")?;
    git(dir.path(), &["add", "file.txt"]);
    git(dir.path(), &["commit", "-m", "multiple hunks"]);
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    press_keys(&mut app, "Inew <esc>jjInew <esc>").await?;
    {
        let (view, doc) = current!(app.editor);
        let end = doc.text().line_to_char_idx(2);
        doc.set_selection(
            view.id,
            Selection::new(vec![Range::point(0), Range::point(end)].into(), 0),
        );
    }
    press_keys(&mut app, "<space>i").await?;
    wait_for_review_screen(&mut app, "Current change", "stage: index").await?;
    press_keys(&mut app, "n").await?;
    finish_partial(&mut app).await?;
    assert_eq!(partial_index(&path), "first\nkeep\nlast\n");
    press_keys(&mut app, "<space>i").await?;
    wait_for_review_screen(&mut app, "Current change", "stage: index").await?;
    press_keys(&mut app, "S").await?;
    finish_partial(&mut app).await?;
    assert_eq!(partial_index(&path), "new first\nkeep\nlast\n");
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.cursor_line(view.id), 2);
    assert_eq!(std::fs::read_to_string(&path)?, "first\nkeep\nlast\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_staging_buffer_keeps_the_bom_of_a_new_utf8_file() -> anyhow::Result<()> {
    let (dir, _path) = committed_file()?;
    let path = dir.path().join("new.txt");
    std::fs::write(&path, "\u{feff}text\n")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    press_keys(&mut app, "Iselected <esc>:vcs-stage-hunk<ret>").await?;
    finish_partial(&mut app).await?;
    assert_eq!(partial_index(&path), "\u{feff}selected text\n");
    assert_eq!(std::fs::read_to_string(&path)?, "\u{feff}text\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

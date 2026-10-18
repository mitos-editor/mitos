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

async fn press_keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    for key in parse_macro(keys)? {
        sender.send(Ok(Event::Key(KeyEvent::from(key))))?;
    }
    let mut input = UnboundedReceiverStream::new(receiver);
    assert!(app.event_loop_until_idle(&mut input).await);
    Ok(())
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
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
        .unwrap();
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

        for _ in 0..2 {
            press_space_b(&mut app).await?;
            assert_eq!(
                app.editor.config().inline_blame.show,
                InlineBlameShow::Never
            );
            press_space_b(&mut app).await?;
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
    use editor_config::DiagnosticFilter;
    use editor_core::diagnostic::{Diagnostic, DiagnosticProvider, Severity};
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
                .is_some_and(|diff| diff.doc().is_instance(doc.text()) && !diff.is_empty())
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
    for keys in ["<space>g", "<space>?changed_file_picker_in_repository<ret>"] {
        press_keys(&mut app, keys).await?;
        let screen: String = app
            .terminal_backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
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

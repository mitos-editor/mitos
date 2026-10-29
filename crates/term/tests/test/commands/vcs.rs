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
        "<space>gc",
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
    press_keys(&mut app, "<space>gcfile.json").await?;
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
    press_keys(&mut app, "<space>gcfile.json").await?;
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
    press_keys(&mut app, "Iunsaved-<esc><space>gcfile.txt").await?;
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
    press_keys(&mut app, "<space>gcfile.txt").await?;
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
    press_keys(&mut app, "<space>gcwatched.txt").await?;
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

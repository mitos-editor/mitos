use std::{path::Path, process::Command, sync::Arc, time::Duration};

use view::{config::InlineBlameShow, current, current_ref};

use term::application::Application;
use tokio_stream::wrappers::UnboundedReceiverStream;
use ui_core::input::parse_macro;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

use super::super::helpers::{run_event_loop_until_idle, test_config, AppBuilder};

async fn press_space_b(app: &mut Application) -> anyhow::Result<()> {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    for key in parse_macro("<space>B")? {
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
async fn narrow_views_keep_error_diagnostics_ahead_of_inline_blame() -> anyhow::Result<()> {
    use editor_core::diagnostic::{Diagnostic, DiagnosticProvider, Severity};
    use view::{annotations::diagnostics::DiagnosticFilter, editor::Action};

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

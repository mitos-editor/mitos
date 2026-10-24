use std::{path::Path, time::Duration};

use super::helpers::{isolation, keys, run_event_loop_until_idle, test_config, AppBuilder};
use term::application::Application;
use view::current_ref;

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
        ])
        .args(args)
        .env_remove("GIT_DIR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn ready(app: &mut Application) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while current_ref!(app.editor).1.review().unwrap().is_loading() {
            run_event_loop_until_idle(app).await;
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn native_review_reopens_picker_and_opens_another_diff_in_a_split() -> anyhow::Result<()> {
    let Some(root) = isolation::workspace(
        "test::git_diff::native_review_reopens_picker_and_opens_another_diff_in_a_split",
    )?
    else {
        return Ok(());
    };
    let workspace = root.join("workspace");
    git(&workspace, &["init"]);
    git(&workspace, &["config", "core.autocrlf", "false"]);
    for name in ["a.txt", "b.txt"] {
        std::fs::write(workspace.join(name), "original\n")?;
    }
    git(&workspace, &["add", "."]);
    git(&workspace, &["commit", "-m", "initial"]);
    std::fs::write(workspace.join("a.txt"), "changed-a\n")?;
    std::fs::write(workspace.join("b.txt"), "changed-b\n")?;
    let mut app = AppBuilder::new()
        .with_file(workspace.join("a.txt"), None)
        .build()?;
    let source = current_ref!(app.editor).1.id();
    keys(&mut app, "<space>ga.txt").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while current_ref!(app.editor).1.review().is_none() {
            keys(&mut app, "<A-o>").await.unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    ready(&mut app).await?;
    let first = current_ref!(app.editor).1.id();
    let first_view = app.editor.tree.focus();
    assert_ne!(first, source);
    assert!(current_ref!(app.editor)
        .1
        .review()
        .unwrap()
        .path()
        .ends_with("a.txt"));

    // Reopening the retained review picker must use the usual split acceptance.
    keys(&mut app, "<space>'<C-a><C-k>b.txt").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while current_ref!(app.editor).1.id() == first {
            keys(&mut app, "<C-v>").await.unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    ready(&mut app).await?;
    let second = current_ref!(app.editor).1.id();
    assert_ne!(first, second);
    assert_eq!(app.editor.tree.views().count(), 2);
    assert_eq!(app.editor.tree.get(first_view).document_id(), first);
    assert!(current_ref!(app.editor)
        .1
        .review()
        .unwrap()
        .path()
        .ends_with("b.txt"));

    // Search, yank, split, and movement are normal editor commands in reviews.
    keys(&mut app, "/changed-b<ret>y").await?;
    assert!(app
        .editor
        .first_register_value('"')
        .unwrap()
        .contains("changed-b"));
    let original_view = app.editor.tree.focus();
    let original_line = current_ref!(app.editor).1.cursor_line(original_view);
    keys(&mut app, "<C-w>vgg").await?;
    assert_eq!(app.editor.tree.views().count(), 3);
    assert_eq!(current_ref!(app.editor).1.id(), second);
    assert_eq!(
        app.editor
            .document(second)
            .unwrap()
            .cursor_line(original_view),
        original_line
    );
    assert_ne!(
        current_ref!(app.editor)
            .1
            .cursor_line(app.editor.tree.focus()),
        original_line
    );
    let before = current_ref!(app.editor).1.text().clone();
    keys(&mut app, "i").await?;
    assert_eq!(app.editor.mode(), view::document::Mode::Normal);
    keys(&mut app, "d:write! forbidden.txt<ret>").await?;
    assert_eq!(current_ref!(app.editor).1.text(), &before);
    assert!(current_ref!(app.editor).1.path().is_none());
    assert!(!workspace.join("forbidden.txt").exists());
    keys(&mut app, ":reload-all<ret>").await?;
    assert!(app.editor.status_msg.is_none());
    assert_eq!(current_ref!(app.editor).1.text(), &before);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_command_creates_a_review_buffer_and_native_history_returns_to_source(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "original\n")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let source = current_ref!(app.editor).1.id();
    keys(&mut app, ":diff<ret>").await?;
    ready(&mut app).await?;
    let review = current_ref!(app.editor).1.id();
    assert_ne!(review, source);
    assert!(current_ref!(app.editor).1.is_content_readonly());
    assert!(current_ref!(app.editor).1.path().is_none());
    keys(&mut app, "<C-o>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), source);
    keys(&mut app, "iedited<esc>").await?;
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .starts_with("edited"));
    assert!(app.editor.document(review).is_some());
    assert_eq!(std::fs::read_to_string(path)?, "original\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[cfg(feature = "syntax-diff")]
#[tokio::test(flavor = "multi_thread")]
async fn native_review_renderer_keeps_unicode_token_styles_and_wrapped_row_backgrounds(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    git(dir.path(), &["init"]);
    git(dir.path(), &["config", "core.autocrlf", "false"]);
    let path = dir.path().join("sample.json");
    let before = format!("{{\"λ\": \"{}\", \"value\": 1}}\n", "longword ".repeat(20));
    std::fs::write(&path, &before)?;
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "initial"]);
    std::fs::write(&path, before.replace(": 1}", ": 2}"))?;
    let mut config = test_config();
    config.editor.soft_wrap.enable = Some(true);
    config.editor.cursor.highlight_line = false;
    config.editor.diff.engine = editor_config::DiffEngine::Syntax;
    let languages = language::Loader::new(
        toml::from_str(
            r#"
        [[language]]
        name = "review-json"
        scope = "source.json"
        file-types = ["json"]
        grammar = "json"
    "#,
        )?,
        Default::default(),
    )?;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(languages)
        .with_file(&path, None)
        .build()?;
    app.editor.theme = theme::Theme::from(toml::from_str::<toml::Value>(
        r##"
        "ui.text" = { fg = "white" }
        "ui.selection" = { bg = "blue" }
        "ui.cursor" = { fg = "black", bg = "yellow" }
        "ui.background" = { bg = "#101010" }
        "diff.plus" = "green"
        "diff.minus" = "red"
        "diff.plus.word" = { fg = "green", modifiers = ["bold"] }
        "diff.minus.word" = { fg = "red", modifiers = ["bold"] }
        "diff.plus.line" = { bg = "#103010" }
        "diff.minus.line" = { bg = "#301010" }
    "##,
    )?);
    keys(&mut app, ":diff<ret>").await?;
    ready(&mut app).await?;
    // Settle the frame requested by the completed review callback.
    run_event_loop_until_idle(&mut app).await;
    assert_eq!(
        current_ref!(app.editor)
            .1
            .review()
            .unwrap()
            .review()
            .provider,
        Some("syntax")
    );
    let buffer = app.terminal_backend().buffer();
    use tui::style::{Color, Modifier};
    assert!(buffer.content.iter().any(|cell| cell.symbol() == "2"
        && cell.fg == Color::Green
        && cell.modifier.contains(Modifier::BOLD)));
    assert!(buffer.content.iter().any(|cell| cell.symbol() == "λ"
        && cell.fg == Color::White
        && !cell.modifier.contains(Modifier::BOLD)));
    let (view, doc) = current_ref!(app.editor);
    let area = view.content_area(doc);
    let gutter_text: Vec<_> = (area.y..area.bottom())
        .map(|y| {
            (area.x..area.x + view.gutter_offset(doc))
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect();
    assert!(
        gutter_text.iter().any(|row| row.contains("1   −")),
        "missing original line number and removal marker: {gutter_text:?}"
    );
    assert!(
        gutter_text.iter().any(|row| row.contains("  1 +")),
        "missing new line number and addition marker: {gutter_text:?}"
    );
    let inner = view.inner_area(doc);
    let added_rows = (inner.y..inner.bottom())
        .filter(|y| buffer[(inner.right() - 1, *y)].bg == Color::Rgb(16, 48, 16))
        .count();
    let removed_rows = (inner.y..inner.bottom())
        .filter(|y| buffer[(inner.right() - 1, *y)].bg == Color::Rgb(48, 16, 16))
        .count();
    assert!(
        added_rows > 1 && removed_rows > 1,
        "wrapped rows must retain diff backgrounds: added {added_rows}, removed {removed_rows}"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

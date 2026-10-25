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
        name = "json"
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
    app.editor
        .set_theme(theme::Theme::from(toml::from_str::<toml::Value>(
            r##"
        "ui.text" = { fg = "white" }
        "ui.selection" = { bg = "blue" }
        "ui.cursor" = { fg = "black", bg = "yellow" }
        "ui.background" = { bg = "#101010" }
        "variable.other.member" = "cyan"
        "string" = "blue"
        "diff.plus" = "green"
        "diff.minus" = "red"
        "diff.plus.word" = { fg = "green", modifiers = ["bold"] }
        "diff.minus.word" = { fg = "red", modifiers = ["bold"] }
        "diff.plus.line" = { bg = "#103010" }
        "diff.minus.line" = { bg = "#301010" }
    "##,
        )?))?;
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
        && cell.fg == Color::Cyan
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

#[tokio::test(flavor = "multi_thread")]
async fn review_syntax_uses_each_source_context_in_both_engines_and_follows_theme_changes(
) -> anyhow::Result<()> {
    use tui::style::Color;
    for (engine, extension) in [
        (editor_config::DiffEngine::Line, "rs"),
        (editor_config::DiffEngine::Syntax, "rs"),
        (editor_config::DiffEngine::Line, "md"),
    ] {
        let dir = tempfile::tempdir()?;
        git(dir.path(), &["init"]);
        git(dir.path(), &["config", "core.autocrlf", "false"]);
        let path = dir.path().join(format!("source.{extension}"));
        // The opening comment is deliberately outside the displayed hunk. The
        // removed declaration is a comment; the added declaration is Rust code.
        let lead: String = (0..10).map(|i| format!("// lead {i}\n")).collect();
        let tail: String = (0..10).map(|i| format!("// tail {i}\n")).collect();
        let before = format!("fn main() {{\n/* opening\n{lead}let old = 1;\n{tail}*/\n}}\n");
        let after = format!("fn main() {{\n/* opening\n{lead}*/\nlet new = 2;\n{tail}}}\n");
        let (before, after) = if extension == "md" {
            (
                format!("```rust\n{before}```\n"),
                format!("```rust\n{after}```\n"),
            )
        } else {
            (before, after)
        };
        std::fs::write(&path, before)?;
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "initial"]);
        std::fs::write(&path, after)?;
        let mut config = test_config();
        config.editor.diff.engine = engine;
        config.editor.cursor.highlight_line = false;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(language::Loader::new(
                toml::from_str(
                    r#"
                [[language]]
                name = "rust"
                scope = "source.rust"
                file-types = ["rs"]
                [[language]]
                name = "markdown"
                scope = "text.markdown"
                file-types = ["md"]
                [[language]]
                name = "markdown.inline"
                scope = "text.markdown.inline"
                grammar = "markdown_inline"
                file-types = []
            "#,
                )?,
                Default::default(),
            )?)
            .with_file(&path, None)
            .build()?;
        let theme = |comment: &str, keyword: &str| -> anyhow::Result<theme::Theme> {
            Ok(theme::Theme::from(toml::from_str::<toml::Value>(
                &format!(
                    r##"
                "ui.text" = "white"
                "ui.selection" = {{ bg = "blue" }}
                "ui.cursor" = {{ fg = "black", bg = "yellow" }}
                "comment" = "{comment}"
                "keyword" = "{keyword}"
                "variable" = "cyan"
                "constant.numeric" = "yellow"
                "diff.plus" = "green"
                "diff.minus" = "red"
                "diff.plus.word" = {{ modifiers = ["bold"] }}
                "diff.minus.word" = {{ modifiers = ["bold"] }}
                "diff.plus.line" = {{ bg = "#103010" }}
                "diff.minus.line" = {{ bg = "#301010" }}
            "##
                ),
            )?))
        };
        app.editor.set_theme(theme("blue", "magenta")?)?;
        keys(&mut app, ":diff<ret>").await?;
        ready(&mut app).await?;
        keys(&mut app, "").await?;
        let assert_colors = |app: &Application, comment: Color, keyword: Color| {
            let buffer = app.terminal_backend().buffer();
            for (contents, color, background) in [
                ("let old", comment, Color::Rgb(48, 16, 16)),
                ("let new", keyword, Color::Rgb(16, 48, 16)),
            ] {
                let row = (0..buffer.area.height)
                    .find_map(|y| {
                        let text: String = (0..buffer.area.width)
                            .map(|x| buffer[(x, y)].symbol())
                            .collect();
                        text.find(contents)
                            .map(|byte| (text[..byte].chars().count() as u16, y))
                    })
                    .unwrap_or_else(|| panic!("Missing {contents:?} in {engine:?} review"));
                let cell = &buffer[row];
                assert_eq!(cell.fg, color, "{contents:?}, {engine:?}");
                assert_eq!(cell.bg, background, "{contents:?}, {engine:?}");
            }
        };
        assert_colors(&app, Color::Blue, Color::Magenta);
        let source = current_ref!(app.editor)
            .1
            .review()
            .unwrap()
            .source()
            .unwrap()
            .clone();
        assert!(source
            .syntax_for_row(
                source
                    .compute(Some(engine), &|| false)
                    .lines
                    .iter()
                    .find(|row| row.text.contains("let old"))
                    .unwrap()
            )
            .is_some());
        app.editor.set_theme(theme("red", "green")?)?;
        keys(&mut app, "").await?;
        assert_colors(&app, Color::Red, Color::Green);
        keys(&mut app, ":diff-engine<ret>").await?;
        ready(&mut app).await?;
        keys(&mut app, "").await?;
        assert_colors(&app, Color::Red, Color::Green);
        assert!(std::sync::Arc::ptr_eq(
            &source,
            current_ref!(app.editor)
                .1
                .review()
                .unwrap()
                .source()
                .unwrap()
        ));
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn review_names_stay_short_in_tabs_and_picker_with_live_details_in_statusline(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    git(dir.path(), &["init"]);
    git(dir.path(), &["config", "core.autocrlf", "false"]);
    let path = dir.path().join("note.txt");
    std::fs::write(&path, "before\n")?;
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "initial"]);
    std::fs::write(&path, "after\n")?;
    let mut config = test_config();
    config.editor.icons = false;
    config.editor.bufferline.show = editor_config::BufferLine::Always;
    config.editor.statusline.left = vec![editor_config::StatusLineElement::FileBaseName];
    config.editor.statusline.right = vec![];
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(language::Loader::default())
        .with_file(&path, None)
        .build()?;
    keys(&mut app, ":diff<ret>").await?;
    ready(&mut app).await?;
    keys(&mut app, "").await?;
    let first = current_ref!(app.editor).1.id();
    let name = current_ref!(app.editor).1.display_name().into_owned();
    assert_eq!(name, "note.txt [diff: HEAD]");
    let row = |app: &Application, y: u16| -> String {
        let buffer = app.terminal_backend().buffer();
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    };
    let tab = row(&app, 0);
    assert!(tab.contains("note.txt [diff: HEAD]"));
    assert!(!tab.contains("line") && !tab.contains("+1") && !tab.contains("loading"));
    let (view, doc) = current_ref!(app.editor);
    let status = row(&app, view.statusline_area(doc).y);
    assert!(status.contains("note.txt") && status.contains("DIFF HEAD"));
    assert!(status.contains("+1") && status.contains("−1") && status.contains("line"));
    keys(&mut app, ":diff-engine<ret>").await?;
    ready(&mut app).await?;
    assert_eq!(current_ref!(app.editor).1.display_name(), name);
    keys(&mut app, ":diff-refresh<ret>").await?;
    ready(&mut app).await?;
    assert_eq!(current_ref!(app.editor).1.display_name(), name);
    keys(&mut app, ":diff<ret>").await?;
    ready(&mut app).await?;
    let second = current_ref!(app.editor).1.id();
    assert!(current_ref!(app.editor)
        .1
        .display_name()
        .ends_with("[diff: HEAD #2]"));
    keys(&mut app, "<C-o>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), first);
    // The picker searches the comparison label and activates the document ID.
    keys(&mut app, "<space>bHEAD #2").await?;
    keys(&mut app, "<ret>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), second);
    assert!(current_ref!(app.editor).1.path().is_none());
    assert!(app.close().await.is_empty());
    Ok(())
}

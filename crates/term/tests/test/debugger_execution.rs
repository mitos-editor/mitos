use std::{path::Path, time::Duration};

use serde_json::json;
use term::application::Application;
use theme::Theme;
use tui::style::Color;
use view::{current_ref, editor::Action};

use super::helpers::{keys, run_event_loop_until_idle, test_config, AppBuilder};

async fn paused(
    path: &Path,
    wrap: bool,
    explicit_style: bool,
) -> anyhow::Result<(Application, dap::DebugAdapterId)> {
    let mut config = test_config();
    config.editor.debugger.inline_values = false;
    config.editor.cursor.highlight_line = true;
    config.editor.soft_wrap.enable = Some(wrap);
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(path, None)
        .with_lang_loader(language::syntax::Loader::default())
        .build()?;
    let mut source = r#"
        "ui.background" = { bg = "black" }
        "ui.text" = { fg = "white" }
        "ui.selection" = { bg = "gray" }
        "ui.cursor" = { bg = "white" }
        "ui.cursorline.primary" = { bg = "green" }
        "ui.highlight" = { bg = "blue" }
    "#
    .to_owned();
    if explicit_style {
        source.push_str("\n\"ui.highlight.frameline\" = { bg = \"yellow\" }");
    }
    app.editor.theme = Theme::from(toml::from_str::<toml::Value>(&source)?);
    let id = app.editor.connect_debugger(
        None,
        language_config::DebugAdapterConfig {
            name: "execution line".into(),
            transport: language_config::DebugAdapterTransport::Stdio,
            command: env!("CARGO_BIN_EXE_mitos-test-dap").into(),
            args: vec![],
            port_arg: None,
            templates: vec![],
            quirks: Default::default(),
        },
        Some(view::debugger::DebuggerLaunch {
            request: dap::ConnectionType::Launch,
            arguments: json!({"inspectVariables":true,"stackFrames":[
                {"id":1,"name":"paused","line":2,"column":1,"source":{"path":path}},
                {"id":2,"name":"caller","line":4,"column":1,"source":{"path":path}}
            ]}),
        }),
    );
    wait_for_frame(&mut app).await?;
    Ok((app, id))
}

async fn wait_for_frame(app: &mut Application) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(app).await;
            if app
                .editor
                .debugger()
                .get_active_session()
                .and_then(|session| session.current_stack_frame())
                .is_some()
            {
                break;
            }
        }
    })
    .await?;
    Ok(())
}

fn highlighted_rows(app: &Application, color: Color) -> usize {
    let backend = app.terminal_backend();
    app.editor
        .tree
        .views()
        .map(|(view, _)| {
            let doc = app.editor.document(view.document_id()).unwrap();
            let area = view.layout(doc, Some(&app.editor.theme)).viewport;
            (area.y..area.bottom())
                .filter(|&y| backend.buffer()[(area.right() - 1, y)].bg == color)
                .count()
        })
        .sum()
}

fn assert_line(app: &Application, text: &str, highlighted: bool) {
    let backend = app.terminal_backend();
    let (view, doc) = current_ref!(app.editor);
    let area = view.layout(doc, Some(&app.editor.theme)).viewport;
    let y = (area.y..area.bottom())
        .find(|&y| {
            let line: String = (area.x..area.right())
                .map(|x| backend.buffer()[(x, y)].symbol())
                .collect();
            line.contains(text)
        })
        .unwrap();
    assert_eq!(
        backend.buffer()[(area.right() - 1, y)].bg == Color::Yellow,
        highlighted,
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn execution_highlight_follows_selected_frame_source_resume_and_termination(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("source.txt");
    std::fs::write(&path, "before\npaused\nbetween\ncaller\nafter\n")?;
    let (mut app, id) = paused(&path, false, true).await?;
    keys(&mut app, "j").await?;
    assert_line(&app, "paused", true);
    assert_line(&app, "between", false);
    keys(&mut app, "<space>Gsfcaller<ret>").await?;
    assert_eq!(
        app.editor
            .debugger()
            .get_active_session()
            .unwrap()
            .current_stack_frame()
            .unwrap()
            .id,
        2
    );
    assert_line(&app, "caller", true);
    assert_line(&app, "paused", false);
    let other = dir.path().join("other.txt");
    std::fs::copy(&path, &other)?;
    app.editor.open(&other, Action::Replace)?;
    app.editor.redraw().request();
    keys(&mut app, "").await?;
    assert_eq!(highlighted_rows(&app, Color::Yellow), 0);
    app.editor.open(&path, Action::Replace)?;
    keys(&mut app, "<space>Gc").await?;
    assert_eq!(highlighted_rows(&app, Color::Yellow), 0);
    app.editor.handle_debugger_message(id, serde_json::from_value(json!({
        "type":"event", "seq":0, "event":"stopped", "body":{"reason":"breakpoint","threadId":1,"allThreadsStopped":true}
    }))?).await;
    wait_for_frame(&mut app).await?;
    assert_line(&app, "paused", true);
    keys(&mut app, "t").await?;
    assert_eq!(highlighted_rows(&app, Color::Yellow), 0);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unsaved_source_hides_execution_highlights_until_undo_restores_the_source(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("source.txt");
    std::fs::write(&path, "before\npaused\nbetween\ncaller\nafter\n")?;
    let (mut app, _) = paused(&path, false, true).await?;
    assert_line(&app, "paused", true);
    keys(&mut app, "Onew line<esc>").await?;
    assert!(current_ref!(app.editor).1.is_modified());
    assert_eq!(highlighted_rows(&app, Color::Yellow), 0);
    keys(&mut app, "u").await?;
    assert!(!current_ref!(app.editor).1.is_modified());
    assert_line(&app, "paused", true);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn wrapped_execution_lines_highlight_each_split_with_the_theme_fallback() -> anyhow::Result<()>
{
    for split in [Action::HorizontalSplit, Action::VerticalSplit] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("source.txt");
        std::fs::write(
            &path,
            format!("before\n{}\nbetween\ncaller\nafter\n", "paused ".repeat(24)),
        )?;
        let (mut app, _) = paused(&path, true, false).await?;
        let document = current_ref!(app.editor).1.id();
        app.editor.switch(document, split);
        app.editor.redraw().request();
        keys(&mut app, "").await?;
        let backend = app.terminal_backend();
        assert_eq!(
            backend
                .buffer()
                .content()
                .iter()
                .filter(|cell| cell.symbol() == "▶")
                .count(),
            2
        );
        for (view, _) in app.editor.tree.views() {
            let doc = app.editor.document(document).unwrap();
            let area = view.layout(doc, Some(&app.editor.theme)).viewport;
            let rows = (area.y..area.bottom())
                .filter(|&y| backend.buffer()[(area.right() - 1, y)].bg == Color::Blue)
                .count();
            assert!(
                rows > 1,
                "wrapped execution line was not highlighted in {:?}",
                area
            );
        }
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

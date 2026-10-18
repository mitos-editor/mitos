use std::time::Duration;

use term::application::Application;
use ui_core::input::parse_macro;
use view::{current_ref, editor::Action, multibuffer::MultiBufferLocation};

use super::helpers::{run_event_loop_until_idle, test_config, AppBuilder};

async fn keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    for key in parse_macro(keys)? {
        #[cfg(not(windows))]
        let event = termina::event::Event::Key(key.into());
        #[cfg(windows)]
        let event = crossterm::event::Event::Key(key.into());
        app.handle_terminal_events(Ok(event)).await;
    }
    app.editor.reset_idle_timer();
    tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(app)).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn picker_matches_open_as_a_multibuffer_and_write_to_sources() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = AppBuilder::new().with_config(test_config()).build()?;
    for (name, text) in [("multi-alpha.txt", "alpha\n"), ("multi-beta.txt", "beta\n")] {
        let path = dir.path().join(name);
        std::fs::write(&path, text)?;
        app.editor.open(&path, Action::Replace)?;
    }
    keys(&mut app, "<space>bmulti-").await?;
    keys(&mut app, "<C-o>").await?;
    let doc = current_ref!(app.editor).1;
    assert_eq!(doc.multibuffer().unwrap().segments().len(), 2);
    assert!(!doc.is_modified());
    let screen: String = app
        .terminal_backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(screen.contains("multi-alpha.txt"), "{screen}");
    assert!(screen.contains("multi-beta.txt"), "{screen}");
    keys(&mut app, "ggIedited <esc>:w<ret>").await?;
    app.editor.flush_writes().await?;
    let files = ["multi-alpha.txt", "multi-beta.txt"]
        .map(|name| std::fs::read_to_string(dir.path().join(name)).unwrap());
    assert_eq!(
        files
            .iter()
            .filter(|text| text.starts_with("edited "))
            .count(),
        1
    );
    assert!(!current_ref!(app.editor).1.is_modified());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn selection_command_expansion_and_source_navigation_work_through_keymaps(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("selected.txt");
    let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&path, text)?;
    let mut app = AppBuilder::new().with_config(test_config()).build()?;
    let source = app.editor.open(&path, Action::Replace)?;
    keys(&mut app, "10G<space>?open_selections_in_multibuffer").await?;
    keys(&mut app, "<ret>").await?;
    assert!(current_ref!(app.editor).1.multibuffer().is_some());
    let before = current_ref!(app.editor).1.text().len_chars();
    keys(&mut app, "<S-ret>").await?;
    assert!(current_ref!(app.editor).1.text().len_chars() > before);
    assert!(!current_ref!(app.editor).1.is_modified());
    keys(&mut app, "<space>?open_multibuffer_excerpt_vsplit").await?;
    keys(&mut app, "<ret>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), source);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn adjacent_unterminated_excerpts_render_and_move_on_distinct_visual_lines(
) -> anyhow::Result<()> {
    for soft_wrap in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut config = test_config();
        config.editor.soft_wrap.enable = Some(soft_wrap);
        let mut app = AppBuilder::new().with_config(config).build()?;
        let mut locations = Vec::new();
        for (name, text) in [("one.txt", "é"), ("two.txt", "🦀 second")] {
            let path = dir.path().join(name);
            std::fs::write(&path, text)?;
            let id = app.editor.open(&path, Action::Replace)?;
            locations.push(MultiBufferLocation::Document(id, None));
        }
        app.editor.open_multibuffer(locations, Action::Replace)?;
        keys(&mut app, "gg").await?;
        keys(&mut app, "j").await?;
        let (view, doc) = current_ref!(app.editor);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            1
        );
        let screen: String = app
            .terminal_backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("one.txt"), "{screen}");
        assert!(screen.contains("two.txt"), "{screen}");
        keys(&mut app, "k").await?;
        let (view, doc) = current_ref!(app.editor);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            0
        );
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

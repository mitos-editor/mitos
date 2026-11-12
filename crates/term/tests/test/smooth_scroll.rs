use super::*;
use term::application::TestApplication;

#[cfg(windows)]
use crossterm::event::Event;
#[cfg(not(windows))]
use termina::event::Event;

const SMOOTH: Option<u64> = Some(180);

type State = (Selection, view::view::ViewPosition);

fn state(app: &TestApplication) -> State {
    let (view, doc) = view::current_ref!(app.editor);
    (doc.selection(view.id).clone(), doc.view_offset(view.id))
}

fn page_app(smooth: Option<u64>, commands: &[&str]) -> anyhow::Result<TestApplication> {
    let text = format!(
        "#[|0]#{}",
        (1..1000).map(|n| format!("\n{n}")).collect::<String>()
    );

    app(smooth, commands, AppBuilder::new().with_input_text(text))
}

fn app(
    smooth: Option<u64>,
    commands: &[&str],
    builder: AppBuilder,
) -> anyhow::Result<TestApplication> {
    let bindings: String = commands
        .iter()
        .enumerate()
        .map(|(i, command)| format!("F{} = \"{command}\"\n", i + 1))
        .collect();
    let mut config = test_config();
    config.editor.scroll.smooth = smooth.is_some();
    if let Some(ms) = smooth {
        config.editor.scroll.scroll_speed = std::time::Duration::from_millis(ms);
    }
    config.keys = Config::load(
        Ok(&format!("[keys.normal]\n{bindings}")),
        Err(term::config::ConfigLoadError::default()),
    )
    .unwrap()
    .keys;

    builder.with_config(config).build()
}

async fn press(app: &mut TestApplication, keys: &str) -> anyhow::Result<()> {
    for key in ui_core::input::parse_macro(keys)? {
        app.handle_terminal_events(Ok(Event::Key(tui::backend::native::adapt(key))))
            .await;
    }

    Ok(())
}

async fn page_state(smooth: Option<u64>, commands: &[&str], keys: &str) -> anyhow::Result<State> {
    let mut app = page_app(smooth, commands)?;
    helpers::keys(&mut app, keys).await?;
    Ok(state(&app))
}

#[tokio::test(flavor = "multi_thread")]
async fn smooth_page_commands_match_instant_commands() -> anyhow::Result<()> {
    for (command, start) in [
        ("page_down", ""),
        ("half_page_down", ""),
        ("page_cursor_down", ""),
        ("page_cursor_half_down", ""),
        ("page_up", "ge"),
        ("half_page_up", "ge"),
        ("page_cursor_up", "ge"),
        ("page_cursor_half_up", "ge"),
    ] {
        let keys = format!("{start}<F1>");
        let expected = page_state(None, &[command], &keys).await?;
        assert_ne!(
            expected,
            page_state(None, &[], start).await?,
            "{command} moved"
        );
        assert_eq!(
            expected,
            page_state(SMOOTH, &[command], &keys).await?,
            "smooth {command}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn smooth_scroll_carries_unfinished_distance() -> anyhow::Result<()> {
    let commands = [
        "page_cursor_down",
        "page_cursor_half_down",
        "page_cursor_half_up",
    ];
    let keys = "<F1><F2><F2><F3>";
    let expected = page_state(None, &commands, keys).await?;
    assert_eq!(expected, page_state(SMOOTH, &commands, keys).await?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn zero_scroll_speed_jumps_to_the_end() -> anyhow::Result<()> {
    let keys = "<F1>";
    let expected = page_state(None, &["page_cursor_half_down"], keys).await?;
    let actual = page_state(Some(0), &["page_cursor_half_down"], keys).await?;
    assert_eq!(expected, actual);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn opposite_smooth_scrolls_cancel_out() -> anyhow::Result<()> {
    let commands = ["page_cursor_half_down", "page_cursor_half_up"];
    let (selection, offset) = page_state(SMOOTH, &commands, "<F1><F2>").await?;
    assert_eq!(
        (selection.primary().from(), offset),
        (0, Default::default())
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn smooth_scroll_leftover_stays_with_its_view() -> anyhow::Result<()> {
    for keys in ["<C-w>v<F1><C-w>w<F1>", "<C-w>v<F1><C-w>q<F1>"] {
        let commands = ["page_cursor_half_down"];
        let expected = page_state(None, &commands, keys).await?;
        let actual = page_state(SMOOTH, &commands, keys).await?;
        assert_eq!(expected, actual, "{keys}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn smooth_scrolls_animate_from_the_old_view() -> anyhow::Result<()> {
    for (command, start, keys) in [
        ("align_view_top", "40gg", "<F1>"),
        ("page_down", "", "<F1>"),
        ("page_cursor_half_down", "", "<F1>"),
        ("page_down", "", "<F1><F1><F1>"),
    ] {
        let commands = [command];
        let expected = page_state(None, &commands, &format!("{start}{keys}")).await?;
        let mut app = page_app(SMOOTH, &commands)?;
        helpers::keys(&mut app, start).await?;
        let before = state(&app);
        press(&mut app, keys).await?;
        assert_eq!(
            before,
            state(&app),
            "{command}: view and cursor must not jump"
        );
        helpers::run_event_loop_until_idle(&mut app).await;
        assert_eq!(expected, state(&app), "{command}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn smooth_view_alignment_matches_instant_alignment() -> anyhow::Result<()> {
    for (command, start) in [
        ("align_view_top", "40gg"),
        ("center_view_vertically", "200ggzt"),
        ("align_view_bottom", "200ggzt"),
    ] {
        let keys = format!("{start}<F1>");
        let expected = page_state(None, &[command], &keys).await?;
        assert_ne!(
            expected,
            page_state(None, &[], start).await?,
            "{command} moved"
        );
        assert_eq!(
            expected,
            page_state(SMOOTH, &[command], &keys).await?,
            "smooth {command}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn smooth_alignment_and_page_scroll_finish_each_other() -> anyhow::Result<()> {
    let commands = ["page_cursor_half_down", "center_view_vertically"];
    for keys in ["<F1><F2>", "200gg<F2><F1>"] {
        let expected = page_state(None, &commands, keys).await?;
        assert_eq!(
            expected,
            page_state(SMOOTH, &commands, keys).await?,
            "{keys}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn smooth_tree_sitter_goto_animates() -> anyhow::Result<()> {
    let commands = ["goto_next_function"];
    let text = format!("#[|f]#n a() {{}}\n{}fn b() {{}}\n", "//\n".repeat(400));
    let rust = || {
        AppBuilder::new()
            .with_file("foo.rs", None)
            .with_input_text(text.clone())
    };

    let mut instant = app(None, &commands, rust())?;
    helpers::run_event_loop_until_idle(&mut instant).await;
    helpers::keys(&mut instant, "<F1>").await?;
    let expected = state(&instant);

    let mut app = app(SMOOTH, &commands, rust())?;
    helpers::run_event_loop_until_idle(&mut app).await;
    let before = state(&app);
    press(&mut app, "<F1>").await?;
    assert_eq!(before, state(&app), "view and cursor must not jump");
    assert_ne!(before, expected, "goto moved");

    helpers::run_event_loop_until_idle(&mut app).await;
    assert_eq!(expected, state(&app));

    Ok(())
}

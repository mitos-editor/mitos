use super::*;

const SMOOTH: Option<u64> = Some(180);

async fn page_state(
    smooth: Option<u64>,
    commands: &[&str],
    keys: &str,
) -> anyhow::Result<(Selection, view::view::ViewPosition)> {
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
    let text = format!(
        "#[|0]#{}",
        (1..300).map(|n| format!("\n{n}")).collect::<String>()
    );
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text(text)
        .build()?;
    helpers::keys(&mut app, keys).await?;
    let (view, doc) = view::current_ref!(app.editor);
    Ok((doc.selection(view.id).clone(), doc.view_offset(view.id)))
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

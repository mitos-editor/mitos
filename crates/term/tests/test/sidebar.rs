use std::path::PathBuf;

use term::application::TestApplication;
use view::{current_ref, editor::Action};

use super::helpers::{keys, test_config, AppBuilder};

fn screen(app: &TestApplication) -> String {
    app.terminal_backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

async fn sidebar() -> anyhow::Result<(TestApplication, tempfile::TempDir, PathBuf)> {
    sidebar_with_config(test_config()).await
}

#[tokio::test(flavor = "multi_thread")]
async fn selection_follows_files_opened_outside_the_sidebar() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::selection_follows_files_opened_outside_the_sidebar",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    keys(&mut app, "/b.txt<ret><ret>").await?;
    let name = root.file_name().unwrap().to_str().unwrap();
    let relative = PathBuf::from(name).join("sub").join("deep").join("d.txt");
    keys(&mut app, &format!("<space>f{}<ret>", relative.display())).await?;
    let nested = root.join("sub").join("deep").join("d.txt");
    assert_eq!(current_ref!(app.editor).1.path(), Some(nested.as_path()));
    for expected in [nested, root.join("b.txt")] {
        keys(&mut app, "<C-w>e<C-x>y<C-e>").await?;
        let selected: Vec<_> = app.editor.registers.read('"').unwrap().collect();
        assert_eq!(
            selected,
            [stdx::path::get_relative_path(&expected).to_string_lossy()]
        );
        keys(&mut app, ":buffer-previous<ret>").await?;
    }
    Ok(())
}

async fn sidebar_with_config(
    config: term::config::Config,
) -> anyhow::Result<(TestApplication, tempfile::TempDir, PathBuf)> {
    // Keep the fixture under the workspace so it can be reached without changing
    // the process-wide working directory of concurrently running tests.
    let dir = tempfile::Builder::new()
        .prefix("sidebar-fixture-")
        .tempdir_in(workspace::ResolvedWorkspace::current().root())?;
    let root = stdx::path::canonicalize(dir.path());
    std::fs::write(root.join("a.txt"), "alpha\n")?;
    std::fs::write(root.join("b.txt"), "bravo\n")?;
    std::fs::create_dir_all(root.join("sub/deep"))?;
    std::fs::write(root.join("sub/c.txt"), "charlie\n")?;
    std::fs::write(root.join("sub/deep/d.txt"), "delta\n")?;
    let mut app = AppBuilder::new().with_config(config).build()?;
    app.editor.open(&root.join("a.txt"), Action::Replace)?;
    keys(&mut app, "<C-w>e").await?;
    let name = root.file_name().unwrap().to_str().unwrap();
    keys(&mut app, &format!("/{name}<ret><ret>")).await?;
    assert!(screen(&app).contains("a.txt"));
    Ok((app, dir, root))
}

#[tokio::test(flavor = "multi_thread")]
async fn enter_confirms_search_and_preserves_results_until_escape() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::enter_confirms_search_and_preserves_results_until_escape",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    keys(&mut app, "/b.txt<ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("a.txt").as_path())
    );
    assert!(!screen(&app).contains("(filter)"));
    assert!(!screen(&app).contains("sub/"));
    keys(&mut app, "jk<ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("b.txt").as_path())
    );
    keys(&mut app, "<C-e>").await?;
    assert!(!screen(&app).contains("sub/"));
    keys(&mut app, "<esc>").await?;
    assert!(screen(&app).contains("sub/"));
    keys(&mut app, "k<ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("a.txt").as_path())
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn confirming_before_results_arrive_survives_focus_and_visibility_changes(
) -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::confirming_before_results_arrive_survives_focus_and_visibility_changes",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    queued_keys(&mut app, "/b.txt<ret><C-e><C-e>q<C-w>b").await?;
    keys(&mut app, "").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("a.txt").as_path())
    );
    assert!(!screen(&app).contains("sub/"));
    keys(&mut app, "<ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("b.txt").as_path())
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_files_keeps_the_sidebar_and_returns_focus_to_the_buffer() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::opening_files_keeps_the_sidebar_and_returns_focus_to_the_buffer",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    let sidebar_width = app.editor.tree.area().x;
    assert!(sidebar_width >= 20);
    keys(&mut app, "/b.txt<ret><ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("b.txt").as_path())
    );
    assert!(screen(&app).contains("File Explorer"));
    assert_eq!(app.editor.tree.area().x, sidebar_width);
    keys(&mut app, "iX<esc>").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "Xbravo\n");
    // Focusing again retains the filter and selected entry.
    keys(&mut app, "<C-w>e<ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("b.txt").as_path())
    );
    keys(&mut app, "<C-w>e<C-x>v").await?;
    assert_eq!(app.editor.tree.views().count(), 2);
    assert!(screen(&app).contains("File Explorer"));
    assert!(app.editor.tree.area().x > 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hiding_and_window_navigation_preserve_splits_and_browser_state() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::hiding_and_window_navigation_preserve_splits_and_browser_state",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    let focused = app.editor.tree.focus();
    // Escape focuses the same document; it does not remove the dock.
    keys(&mut app, "/b.txt").await?;
    keys(&mut app, "<C-e><C-w>v").await?;
    assert_eq!(app.editor.tree.views().count(), 2);
    // First left jump enters the other split, the next enters the dock.
    keys(&mut app, "<C-w>h").await?;
    assert_eq!(app.editor.tree.focus(), focused);
    keys(&mut app, "<C-w>h<C-w>q").await?;
    assert!(!screen(&app).contains("File Explorer"));
    assert_eq!(app.editor.tree.area().x, 0);
    assert_eq!(app.editor.tree.views().count(), 2);
    keys(&mut app, "<C-w>b<ret>").await?;
    assert_eq!(app.editor.tree.views().count(), 2);
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("b.txt").as_path())
    );
    // The regular toggle works while the document owns focus too.
    keys(&mut app, "<C-w>b<C-w>b<C-w>l").await?;
    assert!(screen(&app).contains("File Explorer"));
    assert_eq!(app.editor.tree.focus(), focused);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn file_operation_prompts_return_to_the_docked_browser() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::file_operation_prompts_return_to_the_docked_browser",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    keys(&mut app, "<C-x>ncreated.txt<ret>").await?;
    assert!(root.join("created.txt").is_file());
    assert!(screen(&app).contains("File Explorer"));
    // A cancelled prompt returns to sidebar navigation.
    keys(&mut app, "<C-x>n<esc>/created.txt<ret><ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("created.txt").as_path())
    );
    assert_eq!(app.editor.tree.views().count(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_shortcut_and_filter_editing_preserve_the_document() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::focus_shortcut_and_filter_editing_preserve_the_document",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    let target = app.editor.tree.focus();
    keys(&mut app, "<C-e>iX<esc><C-e>").await?;
    assert_eq!(app.editor.tree.focus(), target);
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "Xalpha\n");
    // Plain navigation keys select rows; they never enter the filter.
    keys(&mut app, "jjk").await?;
    assert!(!screen(&app).contains("(filter)"));
    keys(&mut app, "/b.txt accidental<C-w>").await?;
    assert!(screen(&app).contains("(filter)"));
    assert!(screen(&app).contains("b.txt"));
    // Ctrl-e switches focus from the filter too; the retained selection opens
    // after returning to navigation, without changing the current split.
    keys(&mut app, "<C-e><C-e><ret>").await?;
    assert_eq!(app.editor.tree.focus(), target);
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("b.txt").as_path())
    );
    assert!(app.editor.tree.area().x > 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn window_cycle_includes_sidebar_in_both_directions() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::window_cycle_includes_sidebar_in_both_directions",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, _root) = sidebar().await?;
    let first = app.editor.tree.focus();
    keys(&mut app, "<C-e><C-w>v").await?;
    let last = app.editor.tree.focus();
    assert_ne!(first, last);
    // Last document -> sidebar -> first document -> last document.
    keys(&mut app, "<C-w>w/").await?;
    assert!(screen(&app).contains("(filter)"));
    keys(&mut app, "<C-e><C-e><C-w>w").await?;
    assert_eq!(app.editor.tree.focus(), first);
    keys(&mut app, "<C-w>w").await?;
    assert_eq!(app.editor.tree.focus(), last);
    // First document -> sidebar -> last document when cycling in reverse.
    keys(&mut app, "<C-w>W<C-w>W/").await?;
    assert!(screen(&app).contains("(filter)"));
    keys(&mut app, "<C-e><C-e><C-w>W").await?;
    assert_eq!(app.editor.tree.focus(), last);
    // With the sidebar hidden, the old document-only cycle remains.
    keys(&mut app, "<C-w>b<C-w>w").await?;
    assert_eq!(app.editor.tree.focus(), first);
    assert_eq!(app.editor.tree.area().x, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn sidebar_bindings_and_inherited_file_actions_can_be_remapped() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::sidebar_bindings_and_inherited_file_actions_can_be_remapped",
    )?
    .is_none()
    {
        return Ok(());
    }
    let configured = term::config::Config::load(
        Ok(&r#"
            [keys.normal]
            C-e = "no_op"
            F4 = "switch_sidebar_focus"
            [keys.sidebar]
            C-e = "unbound"
            F4 = "focus_editor"
            [keys.sidebar-filter]
            C-e = "unbound"
            F4 = "focus_editor"
            [keys.file-explorer]
            F2 = "create_path"
        "#
        .to_owned()),
        Err(Default::default()),
    )
    .unwrap();
    let mut config = test_config();
    config.keys = configured.keys;
    config.component_keys = configured.component_keys;
    let (mut app, _dir, root) = sidebar_with_config(config).await?;
    keys(&mut app, "<F2>nav.txt<ret>/b.txt").await?;
    keys(&mut app, "<F4>iX<esc><F4><ret>").await?;
    assert!(root.join("nav.txt").is_file());
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("b.txt").as_path())
    );
    keys(&mut app, "<F4>/b.txt<F2>filter.txt<ret>").await?;
    assert!(root.join("filter.txt").is_file());
    assert!(screen(&app).contains("(filter)"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn visible_sidebar_does_not_keep_the_last_document_window_alive() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::visible_sidebar_does_not_keep_the_last_document_window_alive",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, _root) = sidebar().await?;
    keys(&mut app, "<C-e>:q!<ret>").await?;
    assert!(app.editor.should_close());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn clicking_the_sidebar_cancels_pending_editor_input() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::clicking_the_sidebar_cancels_pending_editor_input",
    )?
    .is_none()
    {
        return Ok(());
    }
    #[cfg(windows)]
    use crossterm::event::{
        Event, KeyModifiers as Modifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    #[cfg(not(windows))]
    use termina::event::{Event, Modifiers, MouseButton, MouseEvent, MouseEventKind};

    let (mut app, _dir, _root) = sidebar().await?;
    keys(&mut app, "<C-e>r").await?;
    app.handle_terminal_events(Ok(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        row: 1,
        column: 1,
        modifiers: Modifiers::empty(),
    })))
    .await;
    keys(&mut app, "<C-e>iX<esc>").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "Xalpha\n");
    assert!(app.editor.tree.area().x > 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn document_rotation_stays_ordered_in_command_sequences() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::document_rotation_stays_ordered_in_command_sequences",
    )?
    .is_none()
    {
        return Ok(());
    }
    let configured = term::config::Config::load(
        Ok(&"[keys.normal]\nF5 = ['rotate_view', 'delete_selection']".to_owned()),
        Err(Default::default()),
    )
    .unwrap();
    let mut config = test_config();
    config.keys = configured.keys;
    let mut app = AppBuilder::new().with_config(config).build()?;
    let dir = tempfile::tempdir()?;
    let first_file = dir.path().join("a.txt");
    let second_file = dir.path().join("b.txt");
    std::fs::write(&first_file, "alpha\n")?;
    std::fs::write(&second_file, "bravo\n")?;
    app.editor.open(&first_file, Action::Replace)?;
    let first = app.editor.tree.focus();
    app.editor.open(&second_file, Action::VerticalSplit)?;
    app.editor.focus(first);
    keys(&mut app, "<F5>").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "ravo\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn returning_after_search_accepts_new_input_and_navigation() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::returning_after_search_accepts_new_input_and_navigation",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    keys(&mut app, "/b.txt<ret><ret><C-w>h<esc>k<ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("a.txt").as_path())
    );
    keys(&mut app, "<C-e>/b.txt<ret><ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("b.txt").as_path())
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn directories_expand_in_place_and_retain_nested_expansion() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::directories_expand_in_place_and_retain_nested_expansion",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    assert!(!screen(&app).contains("c.txt"));
    keys(&mut app, "/^sub$<ret><ret>").await?;
    let expanded = screen(&app);
    assert!(
        expanded.contains("a.txt") && expanded.contains("b.txt"),
        "{expanded}"
    );
    assert!(expanded.contains("c.txt") && expanded.contains("▾ sub/"));
    keys(&mut app, "/^deep$<ret><ret>").await?;
    assert!(screen(&app).contains("d.txt"));
    keys(&mut app, "/^sub$<ret><ret>").await?;
    assert!(!screen(&app).contains("c.txt"));
    assert!(!screen(&app).contains("d.txt"));
    keys(&mut app, "/^sub$<ret><ret>").await?;
    assert!(screen(&app).contains("d.txt"));
    keys(&mut app, "/d.txt<ret><ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("sub/deep/d.txt").as_path())
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn creating_in_the_tree_uses_the_selected_directory_and_refreshes_children(
) -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::creating_in_the_tree_uses_the_selected_directory_and_refreshes_children",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    keys(&mut app, "/^sub$<ret><ret><C-x>ncreated.txt<ret>").await?;
    assert!(root.join("sub/created.txt").is_file(), "{}", screen(&app));
    // File creation can finish before the asynchronous tree refresh is rendered.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !screen(&app).contains("created.txt") {
            keys(&mut app, "").await?;
        }
        anyhow::Ok(())
    })
    .await
    .map_err(|error| {
        anyhow::anyhow!("waiting for created.txt: {error}; screen: {}", screen(&app))
    })??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn dock_has_only_a_right_hand_window_divider() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::dock_has_only_a_right_hand_window_divider",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (app, _dir, _root) = sidebar().await?;
    let buffer = app.terminal_backend().buffer();
    let right = app.editor.tree.area().x - 1;
    for y in 0..app.editor.tree.area().bottom() {
        assert_eq!(buffer[(right, y)].symbol(), "│");
        assert!(!["│", "─", "╭", "╰", "┌", "└"].contains(&buffer[(0, y)].symbol()));
    }
    for x in 0..right {
        assert!(!["─", "╭", "╮", "┌", "┐"].contains(&buffer[(x, 0)].symbol()));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn search_reveals_collapsed_ancestors_and_hides_unmatched_files() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::search_reveals_collapsed_ancestors_and_hides_unmatched_files",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    keys(&mut app, "/d.txt").await?;
    let filtered = screen(&app);
    assert!(filtered.contains("d.txt"), "{filtered}");
    assert!(
        filtered.contains("▾ sub/") && filtered.contains("▾ deep/"),
        "{filtered}"
    );
    for name in ["a.txt", "b.txt", "c.txt"] {
        // Only inspect the sidebar: the bufferline still names a.txt.
        let buffer = app.terminal_backend().buffer();
        let width = app.editor.tree.area().x as usize;
        let sidebar: String = buffer
            .content
            .chunks(buffer.area.width as usize)
            .flat_map(|row| row[..width].iter().map(|cell| cell.symbol()))
            .collect();
        assert!(!sidebar.contains(name), "{sidebar}");
    }
    keys(
        &mut app,
        "<backspace><backspace><backspace><backspace><backspace>zzzznomatch",
    )
    .await?;
    assert!(!screen(&app).contains("▾ sub/"));
    assert!(!screen(&app).contains("▾ deep/"));
    let directory = root.file_name().unwrap().to_string_lossy();
    keys(&mut app, &format!("<C-w>{directory}/sub/deep/d.txt")).await?;
    assert!(screen(&app).contains("▾ deep/"));
    keys(&mut app, "<ret><ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("sub/deep/d.txt").as_path())
    );
    keys(&mut app, "<C-e><esc>").await?;
    assert!(screen(&app).contains("c.txt"));
    assert!(screen(&app).contains("d.txt"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn escape_leaves_search_and_keeps_sidebar_navigation_focused() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::escape_leaves_search_and_keeps_sidebar_navigation_focused",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    keys(&mut app, "/a.txt").await?;
    keys(&mut app, "<esc>").await?;
    assert!(!screen(&app).contains("(filter)"));
    keys(&mut app, "j<ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("b.txt").as_path())
    );
    Ok(())
}

/// Deliver input before the event loop can apply the background search result.
async fn queued_keys(app: &mut TestApplication, input: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    use crossterm::event::Event;
    #[cfg(not(windows))]
    use termina::event::Event;
    for key in ui_core::input::parse_macro(input)? {
        app.handle_terminal_events(Ok(Event::Key(tui::backend::native::adapt(key))))
            .await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn early_accept_preserves_following_input_and_macro_recording() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::early_accept_preserves_following_input_and_macro_recording",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    keys(&mut app, "<C-e>\"aQ<C-e>").await?;
    queued_keys(&mut app, "/b.txt<ret><ret>iX<esc>Q").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("a.txt").as_path())
    );
    keys(&mut app, "").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("b.txt").as_path())
    );
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "Xbravo\n");
    let recorded = app
        .editor
        .registers
        .read('a')
        .unwrap()
        .next()
        .unwrap()
        .into_owned();
    assert_eq!(
        ui_core::input::parse_macro(&recorded)?,
        ui_core::input::parse_macro("<C-e>/b.txt<ret><ret>iX<esc>")?
    );
    keys(&mut app, "\"aq").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "XXbravo\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_pending_search_discards_its_result() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::cancelling_a_pending_search_discards_its_result",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, root) = sidebar().await?;
    queued_keys(&mut app, "/b.txt<esc>/a.txt<ret><ret>iY<esc>").await?;
    keys(&mut app, "").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("a.txt").as_path())
    );
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "Yalpha\n");
    Ok(())
}

// Windows does not allow renaming the process's current directory.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn failed_search_discards_input_queued_for_its_result() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::failed_search_discards_input_queued_for_its_result",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, _root) = sidebar().await?;
    let workspace = workspace::ResolvedWorkspace::current().into_root();
    let moved = workspace.with_file_name("moved-workspace");
    std::fs::rename(&workspace, &moved)?;
    queued_keys(&mut app, "/b.txt<ret><ret><C-e>").await?;
    let result = keys(&mut app, "").await;
    std::fs::rename(&moved, &workspace)?;
    result?;
    assert!(app.editor.status_msg.is_some());

    // A later successful scan must not dispatch the failed search's Ctrl-e
    // and transfer subsequent typing to the document.
    keys(&mut app, "/a.txt<ret>").await?;
    keys(&mut app, "iY<esc>").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "alpha\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn window_prefix_shows_hints_in_the_sidebar_and_clears_them() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::window_prefix_shows_hints_in_the_sidebar_and_clears_them",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, _root) = sidebar().await?;
    keys(&mut app, "<C-w>").await?;
    assert!(screen(&app).contains("Focus document window"));
    assert!(screen(&app).contains("Hide file explorer"));
    // The sidebar has its own available window actions, without split commands.
    assert!(!screen(&app).contains("Split horizontally"));
    for suffix in ["<esc>", "z", "l"] {
        keys(&mut app, suffix).await?;
        assert!(!screen(&app).contains("Focus document window"));
        if suffix == "l" {
            keys(&mut app, "<C-e>").await?;
        }
        keys(&mut app, "<C-w>").await?;
        assert!(screen(&app).contains("Focus document window"));
    }
    keys(&mut app, "q").await?;
    assert!(!screen(&app).contains("Hide file explorer"));
    assert!(!screen(&app).contains("File Explorer"));
    keys(&mut app, ":set auto-info false<ret><C-w>e<C-w>").await?;
    assert!(!screen(&app).contains("Focus document window"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn sidebar_hints_follow_remapped_and_nested_prefixes() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::sidebar_hints_follow_remapped_and_nested_prefixes",
    )?
    .is_none()
    {
        return Ok(());
    }
    let configured = term::config::Config::load(
        Ok(&r#"
            [keys.sidebar.F12]
            x = "focus_editor"
            z = "no_op"
            u = "unbound"
            n = { y = "hide" }
            a = ["filter", "focus_editor"]
            [keys.sidebar-filter.F12]
            q = "focus_editor"
        "#
        .to_owned()),
        Err(Default::default()),
    )
    .unwrap();
    let mut config = test_config();
    config.component_keys = configured.component_keys;
    let (mut app, _dir, _root) = sidebar_with_config(config).await?;
    keys(&mut app, "<F12>").await?;
    let hints = screen(&app);
    assert!(hints.contains("x  Focus document window"));
    assert!(hints.contains("n  More keys"));
    assert!(hints.contains("a  Filter files; Focus document window"));
    assert!(!hints.contains("z  "));
    assert!(!hints.contains("u  "));
    assert!(!hints.contains("Hide file explorer"));
    keys(&mut app, "n").await?;
    assert!(screen(&app).contains("y  Hide file explorer"));
    assert!(!screen(&app).contains("Focus document window"));
    keys(&mut app, "<esc>").await?;
    assert!(!screen(&app).contains("Hide file explorer"));
    // Prefixes also work while editing the explorer's filter.
    keys(&mut app, "/<F12>").await?;
    assert!(screen(&app).contains("q  Focus document window"));
    assert!(!screen(&app).contains("More keys"));
    keys(&mut app, "<esc>").await?;
    assert!(!screen(&app).contains("Focus document window"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn resize_mode_changes_width_and_can_cancel_or_reset() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::resize_mode_changes_width_and_can_cancel_or_reset",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, _root) = sidebar().await?;
    let original = app.editor.tree.area();
    keys(&mut app, "<C-w>r5ll<ret>").await?;
    assert_eq!(app.editor.tree.area().x, original.x + 10);
    assert_eq!(app.editor.tree.area().right(), original.right());
    keys(&mut app, "<C-w>r1h<esc>").await?;
    assert_eq!(app.editor.tree.area().x, original.x + 10);
    keys(&mut app, "q<C-w>e").await?;
    assert_eq!(app.editor.tree.area().x, original.x + 10);
    keys(&mut app, "<C-w>r9999l<ret>").await?;
    assert_eq!(app.editor.tree.area().width, 24);
    keys(&mut app, "<C-w>r9999h<ret>").await?;
    assert_eq!(app.editor.tree.area().x, 20);
    keys(&mut app, "<C-w>r=<ret>").await?;
    assert_eq!(app.editor.tree.area(), original);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn resize_keys_and_counts_match_for_sidebar_and_document() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::resize_keys_and_counts_match_for_sidebar_and_document",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, _root) = sidebar().await?;
    for focus_document in [false, true] {
        if focus_document {
            keys(&mut app, "<C-e>").await?;
        }
        let original = app.editor.tree.area();
        keys(&mut app, "<C-w>r1000000009").await?;
        assert_eq!(
            app.interaction.count.map(std::num::NonZeroUsize::get),
            Some(100_000_000)
        );
        keys(&mut app, "z0").await?;
        assert_eq!(app.interaction.count, None);
        keys(&mut app, "2j").await?;
        assert_eq!(app.interaction.count, None);
        assert_eq!(app.editor.tree.area(), original);
        keys(&mut app, "<esc>").await?;
        // Leading zero is ignored; unsupported keys and vertical moves consume
        // their count, so the following horizontal move uses the default.
        keys(&mut app, "<C-w>r03l2zl2jl<ret>").await?;
        assert_eq!(app.editor.tree.area().x, original.x + 13);
        assert_eq!(app.interaction.count, None);
        keys(&mut app, "<C-w>r10h99<esc>").await?;
        assert_eq!(app.editor.tree.area().x, original.x + 13);
        assert_eq!(app.interaction.count, None);
        assert!(app.editor.autoinfo.is_none());
        keys(&mut app, "<C-w>r=<ret>").await?;
        assert_eq!(app.editor.tree.area(), original);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn document_resize_uses_sidebar_neighbor_and_preserves_split_boundaries() -> anyhow::Result<()>
{
    if super::helpers::isolation::workspace(
        "test::sidebar::document_resize_uses_sidebar_neighbor_and_preserves_split_boundaries",
    )?
    .is_none()
    {
        return Ok(());
    }
    let mut config = test_config();
    let configured = term::config::Config::load(
        Ok(&"[keys.normal]\nF6 = 'resize_view_right'".to_owned()),
        Err(Default::default()),
    )
    .unwrap();
    config.keys = configured.keys;
    let (mut app, _dir, _root) = sidebar_with_config(config).await?;
    keys(&mut app, "<C-e>").await?;
    let original = app.editor.tree.area();
    keys(&mut app, "<C-w>r1h<ret>").await?;
    assert_eq!(app.editor.tree.area().x, original.x - 1);
    keys(&mut app, "<C-w>r1l<esc>").await?;
    assert_eq!(app.editor.tree.area().x, original.x - 1);
    // Direct directional commands use the same boundary dispatch.
    keys(&mut app, "1<F6>").await?;
    assert_eq!(app.editor.tree.area(), original);
    app.editor.new_file(Action::VerticalSplit);
    let sidebar_edge = app.editor.tree.area().x;
    // A regular split boundary stays authoritative, even when constrained.
    keys(&mut app, "<C-w>r99999l<ret>").await?;
    assert_eq!(app.editor.tree.area().x, sidebar_edge);
    keys(&mut app, "<C-w>r99999l<ret>").await?;
    assert_eq!(app.editor.tree.area().x, sidebar_edge);
    // Restore room on both sides, then change the internal split and the
    // sidebar in one interaction. Escape restores both entry snapshots.
    keys(&mut app, "<C-w>r=<ret>").await?;
    let root_before = app.editor.tree.area();
    let left = app
        .editor
        .tree
        .views()
        .min_by_key(|(view, _)| view.area.x)
        .unwrap()
        .0
        .id;
    app.editor.focus(left);
    let panes = |app: &TestApplication| {
        app.editor
            .tree
            .views()
            .map(|(view, _)| view.area)
            .collect::<Vec<_>>()
    };
    let panes_before = panes(&app);
    assert_eq!(current_ref!(app.editor).0.area.x, root_before.x);
    keys(&mut app, "<C-w>r1l").await?;
    assert_eq!(app.editor.tree.area(), root_before);
    assert_ne!(panes(&app), panes_before);
    keys(&mut app, "1h").await?;
    assert_eq!(app.editor.tree.area().x, root_before.x - 1);
    keys(&mut app, "<esc>").await?;
    assert_eq!(app.editor.tree.area(), root_before);
    assert_eq!(panes(&app), panes_before);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_resize_records_and_replays_macros() -> anyhow::Result<()> {
    if super::helpers::isolation::workspace(
        "test::sidebar::shared_resize_records_and_replays_macros",
    )?
    .is_none()
    {
        return Ok(());
    }
    let (mut app, _dir, _root) = sidebar().await?;
    keys(&mut app, "<C-e>").await?;
    let original = app.editor.tree.area().x;
    keys(&mut app, "\"aQ<C-e><C-w>r1l<ret><C-e><C-w>r1l<ret>Q").await?;
    assert_eq!(app.editor.tree.area().x, original + 2);
    keys(&mut app, "\"aq").await?;
    assert_eq!(app.editor.tree.area().x, original + 4);
    assert_eq!(app.interaction.count, None);
    assert!(app.editor.autoinfo.is_none());
    Ok(())
}

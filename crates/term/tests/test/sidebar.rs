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

async fn sidebar_with_config(
    config: term::config::Config,
) -> anyhow::Result<(TestApplication, tempfile::TempDir, PathBuf)> {
    // Keep the fixture under the workspace so it can be reached without changing
    // the process-wide working directory of concurrently running tests.
    let dir = tempfile::Builder::new()
        .prefix("sidebar-fixture-")
        .tempdir_in(workspace::find_workspace().0)?;
    let root = stdx::path::canonicalize(dir.path());
    std::fs::write(root.join("a.txt"), "alpha\n")?;
    std::fs::write(root.join("b.txt"), "bravo\n")?;
    let mut app = AppBuilder::new().with_config(config).build()?;
    app.editor.open(&root.join("a.txt"), Action::Replace)?;
    keys(&mut app, "<C-w>e").await?;
    let name = root.file_name().unwrap().to_str().unwrap();
    keys(&mut app, &format!("/{name}<ret>")).await?;
    assert!(screen(&app).contains("a.txt"));
    Ok((app, dir, root))
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_files_keeps_the_sidebar_and_returns_focus_to_the_buffer() -> anyhow::Result<()> {
    let (mut app, _dir, root) = sidebar().await?;
    let sidebar_width = app.editor.tree.area().x;
    assert!(sidebar_width >= 20);
    keys(&mut app, "/b.txt<ret>").await?;
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
    keys(&mut app, "<C-w>e<C-v>").await?;
    assert_eq!(app.editor.tree.views().count(), 2);
    assert!(screen(&app).contains("File Explorer"));
    assert!(app.editor.tree.area().x > 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hiding_and_window_navigation_preserve_splits_and_browser_state() -> anyhow::Result<()> {
    let (mut app, _dir, root) = sidebar().await?;
    let focused = app.editor.tree.focus();
    // Escape focuses the same document; it does not remove the dock.
    keys(&mut app, "/b.txt<esc><C-w>v").await?;
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
    let (mut app, _dir, root) = sidebar().await?;
    keys(&mut app, "<A-n>created.txt<ret>").await?;
    assert!(root.join("created.txt").is_file());
    assert!(screen(&app).contains("File Explorer"));
    // A cancelled prompt returns to sidebar navigation.
    keys(&mut app, "<A-n><esc>/created.txt<ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("created.txt").as_path())
    );
    assert_eq!(app.editor.tree.views().count(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_shortcut_and_filter_editing_preserve_the_document() -> anyhow::Result<()> {
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
    keys(&mut app, "<F2>nav.txt<ret>/b.txt<F4>iX<esc><F4><ret>").await?;
    assert!(root.join("nav.txt").is_file());
    assert_eq!(
        current_ref!(app.editor).1.path(),
        Some(root.join("b.txt").as_path())
    );
    keys(&mut app, "<F4>/<F2>filter.txt<ret>").await?;
    assert!(root.join("filter.txt").is_file());
    assert!(screen(&app).contains("(filter)"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn visible_sidebar_does_not_keep_the_last_document_window_alive() -> anyhow::Result<()> {
    let (mut app, _dir, _root) = sidebar().await?;
    keys(&mut app, "<C-e>:q!<ret>").await?;
    assert!(app.editor.should_close());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn clicking_the_sidebar_cancels_pending_editor_input() -> anyhow::Result<()> {
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

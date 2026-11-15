use super::helpers::{
    lsp::{self, ServerConfig},
    test_config,
};
use anyhow::Context as _;
#[cfg(windows)]
use crossterm::event as native;
use std::{path::Path, time::Duration};
use term::application::TestApplication;
#[cfg(not(windows))]
use termina::event as native;
use view::{
    current_ref,
    editor::Action,
    features::inlay_hints::{self, InlayHintsHandler},
    ViewId,
};

async fn fixture(
    dir: &Path,
    mixed: bool,
    truncated: bool,
) -> anyhow::Result<(TestApplication, ViewId)> {
    fixture_args(dir, mixed, truncated, &[]).await
}

async fn fixture_args(
    dir: &Path,
    mixed: bool,
    truncated: bool,
    args: &[&str],
) -> anyhow::Result<(TestApplication, ViewId)> {
    let mut config = test_config();
    config.editor.lsp.inlay_hints.enable = true;
    config.editor.lsp.inlay_hints.length_limit = truncated.then(|| 3.try_into().unwrap());
    config.editor.completion.word.enable = args.contains(&"--mouse-word-completion");
    config.editor.completion.path.enable = false;
    config.editor.mouse.yank_register = 'a';
    let (mut app, mut callbacks, gate) = lsp::feature_app(
        dir,
        "😀 original\n",
        "hover-test",
        &["alpha"],
        config,
        |name| {
            let mut server =
                ServerConfig::feature(name, "--language-features", dir).arg(if mixed {
                    "--mixed-inlays"
                } else {
                    "--inlay-details"
                });
            for arg in args {
                server = server.arg(arg);
            }
            server
        },
        |handlers, sender, _| handlers.inlay_hints = InlayHintsHandler::new(sender),
    )?;
    gate.release()?;
    lsp::initialize(&mut app, 1).await?;
    inlay_hints::refresh(&mut app.editor);
    let (_, callback) = tokio::time::timeout(Duration::from_secs(5), callbacks.recv())
        .await?
        .context("missing inlay callback")?;
    callback(&mut app.editor);
    let view = current_ref!(app.editor).0.id;
    Ok((app, view))
}

fn point(app: &TestApplication, view: ViewId, column: u16) -> (u16, u16) {
    let view = app.editor.tree.get(view);
    let area = view.inner_area(app.editor.document(view.document_id()).unwrap());
    (area.y, area.x + column)
}

#[tokio::test(flavor = "multi_thread")]
async fn displayed_parts_map_wide_and_combining_cells_without_padding_or_ellipsis(
) -> anyhow::Result<()> {
    for truncated in [false, true] {
        let dir = tempfile::tempdir()?;
        let (app, view) = fixture(dir.path(), false, truncated).await?;
        let hit = |column| {
            let (row, column) = point(&app, view, column);
            inlay_hints::hint_at_screen_coords(&app.editor, view, row, column)
        };
        let first = hit(4).context("missing first label part")?;
        assert_eq!(first.label_part, Some(0));
        for column in [5, 6] {
            let same = hit(column).context("missing wide/combining label cell")?;
            assert!(first.target.same_hint(&same.target));
            assert_eq!(same.label_part, Some(0));
        }
        assert!(hit(3).is_none(), "padding is not a hint action target");
        if truncated {
            assert!(
                hit(7).is_none(),
                "generated ellipsis is not original label text"
            );
            assert!(hit(8).is_none());
        } else {
            assert_eq!(hit(7).unwrap().label_part, Some(1));
            assert!(hit(14).is_none());
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn same_anchor_hints_keep_distinct_mouse_identities_in_response_order() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let (app, view) = fixture(dir.path(), true, false).await?;
    let hits = [4, 7, 8].map(|column| {
        let (row, column) = point(&app, view, column);
        inlay_hints::hint_at_screen_coords(&app.editor, view, row, column).unwrap()
    });
    assert!(hits
        .iter()
        .all(|hit| hit.target.is_current(&app.editor) && hit.label_part.is_none()));
    assert!(!hits[0].target.same_hint(&hits[1].target));
    assert!(!hits[1].target.same_hint(&hits[2].target));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn passive_hits_preserve_focus_and_selection_and_expire_on_focus_or_cache_changes(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, origin) = fixture(dir.path(), false, false).await?;
    let document = app.editor.tree.get(origin).document_id();
    app.editor.new_file(Action::VerticalSplit);
    let focused = app.editor.tree.focus();
    let original_selection = app
        .editor
        .document(document)
        .unwrap()
        .selection(origin)
        .clone();
    let focused_selection = current_ref!(app.editor).1.selection(focused).clone();
    let (row, column) = point(&app, origin, 4);
    let hit = inlay_hints::hint_at_screen_coords(&app.editor, origin, row, column)
        .context("missing unfocused hit")?;
    assert!(hit.target.is_current(&app.editor));
    assert_eq!(app.editor.tree.focus(), focused);
    assert_eq!(
        app.editor.document(document).unwrap().selection(origin),
        &original_selection
    );
    assert_eq!(
        current_ref!(app.editor).1.selection(focused),
        &focused_selection
    );
    let resolved = inlay_hints::resolve(&mut app.editor, hit.target.clone())
        .unwrap()
        .await
        .unwrap();
    assert!(resolved.is_current(&app.editor));
    for action in resolved.actions() {
        assert!(resolved
            .execute(&mut app.editor, action, Action::Replace)
            .is_err());
    }
    assert!(!lsp::read_log(&lsp::log_path(dir.path(), "alpha"))?
        .iter()
        .any(|message| message["method"] == "workspace/executeCommand"));
    app.editor.focus(origin);
    assert!(!hit.target.is_current(&app.editor));
    let (row, column) = point(&app, origin, 4);
    let hit = inlay_hints::hint_at_screen_coords(&app.editor, origin, row, column).unwrap();
    let server = app
        .editor
        .document(document)
        .unwrap()
        .language_servers()
        .next()
        .unwrap()
        .id();
    inlay_hints::refresh_for_language_server(&mut app.editor, server);
    assert!(!hit.target.is_current(&app.editor));
    assert!(inlay_hints::hint_at_screen_coords(&app.editor, origin, row, column).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_hover_is_retired_when_layout_or_terminal_focus_changes() -> anyhow::Result<()> {
    use super::helpers::{keys, screen};
    for resize in [false, true] {
        let dir = tempfile::tempdir()?;
        let gate = lsp::Gate::new(dir.path().join("resolve"));
        let (mut app, view) = fixture_args(
            dir.path(),
            false,
            false,
            &["--resolve-gate", gate.path().to_str().unwrap()],
        )
        .await?;
        let hit = point(&app, view, 4);
        move_to(&mut app, hit).await?;
        lsp::wait_message(&lsp::log_path(dir.path(), "alpha"), "inlayHint/resolve").await?;
        #[cfg(windows)]
        let resize_event = native::Event::Resize(60, 20);
        #[cfg(not(windows))]
        let resize_event = native::Event::WindowResized(termina::WindowSize {
            rows: 20,
            cols: 60,
            pixel_width: None,
            pixel_height: None,
        });
        #[cfg(windows)]
        let focus_lost = native::Event::FocusLost;
        #[cfg(not(windows))]
        let focus_lost = native::Event::FocusOut;
        let event = if resize { resize_event } else { focus_lost };
        app.handle_terminal_events(Ok(event)).await;
        keys(&mut app, "").await?;
        gate.release()?;
        keys(&mut app, "").await?;
        assert!(!screen(&app).contains("literal whole hint"));
        assert!(!screen(&app).contains("Loading inlay hint"));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_completion_preview_invalidates_an_existing_hint_target() -> anyhow::Result<()> {
    use super::helpers::keys;
    let dir = tempfile::tempdir()?;
    let (mut app, view) =
        fixture_args(dir.path(), false, false, &["--mouse-word-completion"]).await?;
    let document = app.editor.tree.get(view).document_id();
    app.editor
        .document_mut(document)
        .unwrap()
        .set_selection(view, editor_core::Selection::point(3));
    app.editor.set_mode(view::document::Mode::Insert);
    let hit = point(&app, view, 4);
    let target = inlay_hints::hint_at_screen_coords(&app.editor, view, hit.0, hit.1)
        .unwrap()
        .target;
    assert!(target.is_current(&app.editor));
    keys(&mut app, "<C-x>").await?;
    keys(&mut app, "<down>").await?;
    assert!(view::features::completion::has_preview(&app.editor));
    assert!(!target.is_current(&app.editor));
    assert!(inlay_hints::hint_at_screen_coords(&app.editor, view, hit.0, hit.1).is_none());
    Ok(())
}

async fn mouse(
    app: &mut TestApplication,
    kind: native::MouseEventKind,
    row: u16,
    column: u16,
    modifiers: ui_core::keyboard::KeyModifiers,
) -> anyhow::Result<()> {
    app.handle_terminal_events(Ok(native::Event::Mouse(native::MouseEvent {
        kind,
        row,
        column,
        modifiers: tui::backend::native::adapt(modifiers),
    })))
    .await;
    super::helpers::keys(app, "").await
}

#[tokio::test(flavor = "multi_thread")]
async fn returning_to_a_document_does_not_revive_its_old_hint_target() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, view) = fixture(dir.path(), false, false).await?;
    let document = current_ref!(app.editor).1.id();
    let hit = point(&app, view, 4);
    let old = inlay_hints::hint_at_screen_coords(&app.editor, view, hit.0, hit.1).unwrap();
    app.editor.new_file(Action::Replace);
    app.editor.switch(document, Action::Replace);
    assert!(!old.target.is_current(&app.editor));
    let fresh = inlay_hints::hint_at_screen_coords(&app.editor, view, hit.0, hit.1).unwrap();
    assert!(fresh.target.is_current(&app.editor));
    assert!(!old.target.same_hint(&fresh.target));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_text_drag_recovers_when_a_hint_click_lost_its_release() -> anyhow::Result<()> {
    use super::helpers::keys;
    use native::{MouseButton, MouseEventKind};
    use ui_core::keyboard::KeyModifiers;
    let dir = tempfile::tempdir()?;
    let (mut app, view) = fixture(dir.path(), true, false).await?;
    let hit = point(&app, view, 4);
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        hit.0,
        hit.1,
        KeyModifiers::empty(),
    )
    .await?;
    keys(&mut app, "<esc>").await?;
    let start = point(&app, view, 0);
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        start.0,
        start.1,
        KeyModifiers::empty(),
    )
    .await?;
    let end = point(&app, view, 25);
    mouse(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        end.0,
        end.1,
        KeyModifiers::empty(),
    )
    .await?;
    let selection = current_ref!(app.editor).1.selection(view).primary();
    assert_ne!(
        selection.anchor, selection.head,
        "the new gesture must select ordinary text"
    );
    Ok(())
}

async fn move_to(app: &mut TestApplication, point: (u16, u16)) -> anyhow::Result<()> {
    mouse(
        app,
        native::MouseEventKind::Moved,
        point.0,
        point.1,
        ui_core::keyboard::KeyModifiers::empty(),
    )
    .await
}

fn text_point(app: &TestApplication, text: &str) -> Option<(u16, u16)> {
    let buffer = app.terminal_backend().buffer();
    let expected = text.chars().map(|ch| ch.to_string()).collect::<Vec<_>>();
    buffer
        .content
        .chunks(usize::from(buffer.area.width))
        .enumerate()
        .find_map(|(row, cells)| {
            cells
                .windows(expected.len())
                .position(|cells| {
                    cells
                        .iter()
                        .zip(&expected)
                        .all(|(cell, expected)| cell.symbol() == expected)
                })
                .map(|column| (row as u16, column as u16))
        })
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_is_passive_deduplicated_and_readable_until_pointer_leaves() -> anyhow::Result<()> {
    use super::helpers::{keys, screen};
    let dir = tempfile::tempdir()?;
    let gate = lsp::Gate::new(dir.path().join("resolve"));
    let (mut app, view) = fixture_args(
        dir.path(),
        false,
        false,
        &["--resolve-gate", gate.path().to_str().unwrap()],
    )
    .await?;
    let document = current_ref!(app.editor).1.id();
    let original = app.editor.document(document).unwrap().text().clone();
    let selection = app
        .editor
        .document(document)
        .unwrap()
        .selection(view)
        .clone();
    app.editor.new_file(Action::HorizontalSplit);
    let focus = app.editor.tree.focus();
    for col in [4, 5, 6] {
        let hit = point(&app, view, col);
        move_to(&mut app, hit).await?;
    }
    lsp::wait_message(&lsp::log_path(dir.path(), "alpha"), "inlayHint/resolve").await?;
    assert_eq!(
        lsp::read_log(&lsp::log_path(dir.path(), "alpha"))?
            .iter()
            .filter(|m| m["method"] == "inlayHint/resolve")
            .count(),
        1
    );
    gate.release()?;
    keys(&mut app, "").await?;
    assert!(
        screen(&app).contains("literal whole hint"),
        "{}",
        screen(&app)
    );
    let inside = text_point(&app, "literal whole hint").unwrap();
    assert!(
        inside.0
            < app
                .editor
                .tree
                .get(focus)
                .inner_area(current_ref!(app.editor).1)
                .top(),
        "passive details belong next to the hit in the unfocused upper split"
    );
    move_to(&mut app, inside).await?;
    assert!(screen(&app).contains("literal whole hint"));
    assert_eq!(app.editor.tree.focus(), focus);
    assert_eq!(
        app.editor.document(document).unwrap().selection(view),
        &selection
    );
    assert_eq!(app.editor.document(document).unwrap().text(), &original);
    assert!(!lsp::read_log(&lsp::log_path(dir.path(), "alpha"))?
        .iter()
        .any(|m| m["method"] == "workspace/executeCommand"));
    let away = point(&app, view, 0);
    move_to(&mut app, away).await?;
    assert!(!screen(&app).contains("literal whole hint"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_hover_escape_leaves_insert_mode_and_rejects_the_late_reply() -> anyhow::Result<()>
{
    use super::helpers::{keys, screen};
    let dir = tempfile::tempdir()?;
    let gate = lsp::Gate::new(dir.path().join("resolve"));
    let (mut app, view) = fixture_args(
        dir.path(),
        false,
        false,
        &["--resolve-gate", gate.path().to_str().unwrap()],
    )
    .await?;
    app.editor.set_mode(view::document::Mode::Insert);
    let hit = point(&app, view, 4);
    move_to(&mut app, hit).await?;
    lsp::wait_message(&lsp::log_path(dir.path(), "alpha"), "inlayHint/resolve").await?;
    keys(&mut app, "<esc>").await?;
    assert_eq!(app.editor.mode(), view::document::Mode::Normal);
    gate.release()?;
    keys(&mut app, "").await?;
    assert!(!screen(&app).contains("literal whole hint"));
    assert!(!screen(&app).contains("Loading inlay hint"));
    lsp::wait_message(&lsp::log_path(dir.path(), "alpha"), "$/cancelRequest").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn modal_layers_do_not_gain_late_hover_popups_or_poison_future_hover() -> anyhow::Result<()> {
    use super::helpers::{keys, screen};
    let dir = tempfile::tempdir()?;
    let gate = lsp::Gate::new(dir.path().join("resolve"));
    let (mut app, view) = fixture_args(
        dir.path(),
        false,
        false,
        &["--resolve-gate", gate.path().to_str().unwrap()],
    )
    .await?;
    let hit = point(&app, view, 4);
    move_to(&mut app, hit).await?;
    lsp::wait_message(&lsp::log_path(dir.path(), "alpha"), "inlayHint/resolve").await?;
    // Use the UI queue to install a modal without a keyboard event dismissing
    // the existing loading layer first.
    app.job_sender()
        .send_callback(term::job::Callback::ui(|compositor, _| {
            compositor.push(Box::new(
                term::ui::Popup::new(
                    "mouse-test-modal",
                    term::ui::Text::new("Unrelated modal".into()),
                )
                .auto_close(true),
            ));
        }))
        .await;
    keys(&mut app, "").await?;
    gate.release()?;
    keys(&mut app, "").await?;
    assert!(screen(&app).contains("Unrelated modal"));
    assert!(!screen(&app).contains("literal whole hint"));
    move_to(&mut app, hit).await?;
    keys(&mut app, "<esc>").await?;
    move_to(&mut app, hit).await?;
    assert!(
        screen(&app).contains("literal whole hint"),
        "{}",
        screen(&app)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plain_click_uses_only_the_selected_parts_actions_without_changing_text(
) -> anyhow::Result<()> {
    use super::helpers::{keys, screen};
    use native::{MouseButton, MouseEventKind};
    use ui_core::keyboard::KeyModifiers;
    let dir = tempfile::tempdir()?;
    let (mut app, view) =
        fixture_args(dir.path(), false, false, &["--inlay-second-action"]).await?;
    let document = current_ref!(app.editor).1.id();
    app.editor
        .document_mut(document)
        .unwrap()
        .set_selection(view, editor_core::Selection::single(2, 5));
    app.editor.registers.write('a', vec!["sentinel".into()])?;
    let original = current_ref!(app.editor).1.text().clone();
    let selection = current_ref!(app.editor).1.selection(view).clone();
    let hit = point(&app, view, 8);
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        hit.0,
        hit.1,
        KeyModifiers::empty(),
    )
    .await?;
    let away = point(&app, view, 0);
    mouse(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        away.0,
        away.1,
        KeyModifiers::empty(),
    )
    .await?;
    #[cfg(not(windows))]
    let focus_events = [native::Event::FocusOut, native::Event::FocusIn];
    #[cfg(windows)]
    let focus_events = [native::Event::FocusLost, native::Event::FocusGained];
    for event in focus_events {
        app.handle_terminal_events(Ok(event)).await;
    }
    mouse(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        hit.0,
        hit.1,
        KeyModifiers::empty(),
    )
    .await?;
    let shown = screen(&app);
    assert!(shown.contains("Inspect suffix"), "{shown}");
    assert!(!shown.contains("Run Inspect ("), "{shown}");
    assert!(shown.contains("Apply hint text edits"));
    assert_eq!(current_ref!(app.editor).1.text(), &original);
    assert_eq!(current_ref!(app.editor).1.selection(view), &selection);
    assert_eq!(
        app.editor.registers.read('a').unwrap().next().unwrap(),
        "sentinel"
    );
    keys(&mut app, "<esc>").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ctrl_click_prefers_location_and_falls_back_to_command_without_applying_edits(
) -> anyhow::Result<()> {
    use native::{MouseButton, MouseEventKind};
    use ui_core::keyboard::KeyModifiers;
    for command_only in [false, true] {
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("target.txt");
        std::fs::write(&target, "😀 target\n")?;
        let uri = editor_core::Uri::from(target.clone())
            .to_url()
            .unwrap()
            .to_string();
        let mut args = vec!["--inlay-target", &uri];
        if command_only {
            args.push("--inlay-command-only");
        }
        let (mut app, view) = fixture_args(dir.path(), false, false, &args).await?;
        let original_doc = current_ref!(app.editor).1.id();
        let original = current_ref!(app.editor).1.text().clone();
        let hit = point(&app, view, 4);
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            hit.0,
            hit.1,
            KeyModifiers::CONTROL,
        )
        .await?;
        if command_only {
            let command = lsp::wait_message(
                &lsp::log_path(dir.path(), "alpha"),
                "workspace/executeCommand",
            )
            .await?;
            assert_eq!(command["params"]["command"], "inlay.inspect");
            assert_eq!(current_ref!(app.editor).1.id(), original_doc);
        } else {
            assert_eq!(current_ref!(app.editor).1.path(), Some(target.as_path()));
            assert!(!lsp::read_log(&lsp::log_path(dir.path(), "alpha"))?
                .iter()
                .any(|m| m["method"] == "workspace/executeCommand"));
        }
        assert_eq!(app.editor.document(original_doc).unwrap().text(), &original);
    }
    Ok(())
}

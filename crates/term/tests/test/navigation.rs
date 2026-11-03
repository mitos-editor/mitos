//! The terminal adapter presents the shared navigation result.
use super::helpers::{
    lsp::{self, ServerConfig},
    test_config,
};
use editor_core::Range;
use std::time::Duration;
use view::{
    current_ref,
    features::navigation::{self, NavigationHandler, NavigationKind},
};

#[tokio::test(flavor = "multi_thread")]
async fn navigation_events_jump_to_the_resolved_symbol() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, mut callbacks, gate) = lsp::feature_app(
        dir.path(),
        "😀 foo foo\n",
        "navigation-test",
        &["alpha"],
        test_config(),
        |name| ServerConfig::feature(name, "--navigation", dir.path()),
        |handlers, sender, _| handlers.navigation = NavigationHandler::new(sender),
    )?;
    gate.release()?;
    lsp::initialize(&mut app, 1).await?;
    // The shared feature captures the current selection; begin at the same cursor as the navigation contract.
    let (view, doc) = view::current!(app.editor);
    doc.set_selection(view.id, editor_core::Selection::point(0));
    navigation::request(&mut app.editor, NavigationKind::Definition);
    let (_, callback) = tokio::time::timeout(Duration::from_secs(5), callbacks.recv())
        .await?
        .unwrap();
    callback(&mut app.editor);
    let update = navigation::next_update(&app.editor).unwrap();
    app.handle_editor_event(view::editor::EditorEvent::Navigation(update))
        .await;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.selection(view.id).primary(), Range::new(5, 2));
    Ok(())
}

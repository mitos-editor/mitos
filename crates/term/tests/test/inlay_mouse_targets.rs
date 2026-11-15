//! Exact displayed inlay labels retain fresh provider-owned interaction targets.
use super::helpers::{
    lsp::{self, ServerConfig},
    test_config,
};
use anyhow::Context as _;
use std::{path::Path, time::Duration};
use term::application::TestApplication;
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
    let mut config = test_config();
    config.editor.lsp.inlay_hints.enable = true;
    config.editor.lsp.inlay_hints.length_limit = truncated.then(|| 3.try_into().unwrap());
    let (mut app, mut callbacks, gate) = lsp::feature_app(
        dir,
        "😀 original\n",
        "hover-test",
        &["alpha"],
        config,
        |name| {
            ServerConfig::feature(name, "--language-features", dir).arg(if mixed {
                "--mixed-inlays"
            } else {
                "--inlay-details"
            })
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

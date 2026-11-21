use super::super::helpers::{self, AppBuilder};
use editor_config::BufferLine;
use view::{current_ref, editor::Action, DocumentId};

fn order(app: &term::application::TestApplication) -> Vec<DocumentId> {
    app.editor.documents().map(|doc| doc.id()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_moves_preserve_views_and_match_bufferline_and_navigation() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    for name in ["first.txt", "second.txt", "third.txt"] {
        std::fs::write(dir.path().join(name), "content\n")?;
    }
    let mut config = helpers::test_config();
    config.editor.bufferline.show = BufferLine::Always;
    config.editor.icons = false;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(language::syntax::Loader::default())
        .with_file(dir.path().join("first.txt"), None)
        .build()?;
    let first = current_ref!(app.editor).1.id();
    let second = app
        .editor
        .open(&dir.path().join("second.txt"), Action::VerticalSplit)?;
    let third = app
        .editor
        .open(&dir.path().join("third.txt"), Action::Load)?;
    let focus = app.editor.tree.focus();
    let views: Vec<_> = app
        .editor
        .tree
        .views()
        .map(|(view, _)| {
            (
                view.id,
                view.document_id(),
                app.editor.documents[&view.document_id()]
                    .selection(view.id)
                    .clone(),
            )
        })
        .collect();
    let focused_at = app.editor.document(second).unwrap().focused_at;

    for (command, expected) in [
        ("buffer-move-previous", [second, first, third]),
        ("buffer-move-previous", [second, first, third]),
        ("buffer-move-next", [first, second, third]),
        ("buffer-move-end", [first, third, second]),
        ("buffer-move-next", [first, third, second]),
        ("buffer-move-start", [second, first, third]),
    ] {
        helpers::keys(&mut app, &format!(":{command}<ret>")).await?;
        helpers::assert_status_not_error(&app.editor);
        assert_eq!(order(&app), expected, "{command}");
        assert_eq!(app.editor.tree.focus(), focus);
        assert_eq!(app.editor.document(second).unwrap().focused_at, focused_at);
        for (id, document, selection) in &views {
            let view = app.editor.tree.get(*id);
            assert_eq!(view.document_id(), *document);
            assert_eq!(app.editor.documents[document].selection(*id), selection);
        }
        let buffer = app.terminal_backend().buffer();
        let row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, 0)].symbol())
            .collect();
        let positions: Vec<_> = expected
            .iter()
            .map(|id| {
                let doc = app.editor.document(*id).unwrap();
                let name = doc.path().unwrap().file_name().unwrap().to_str().unwrap();
                row.find(name).unwrap()
            })
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{row}");
    }
    helpers::keys(&mut app, ":buffer-next<ret>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), first);
    helpers::keys(&mut app, ":buffer-previous<ret>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), second);
    helpers::keys(&mut app, ":buffer-previous<ret>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), third);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_order_survives_closing_and_opening_documents() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    for name in ["first.txt", "second.txt", "third.txt", "fourth.txt"] {
        std::fs::write(dir.path().join(name), "content\n")?;
    }
    let mut app = AppBuilder::new()
        .with_lang_loader(language::syntax::Loader::default())
        .with_file(dir.path().join("first.txt"), None)
        .build()?;
    let first = current_ref!(app.editor).1.id();
    let second = app
        .editor
        .open(&dir.path().join("second.txt"), Action::Load)?;
    let third = app
        .editor
        .open(&dir.path().join("third.txt"), Action::Load)?;
    helpers::keys(&mut app, ":buffer-move-end<ret>").await?;
    assert_eq!(order(&app), [second, third, first]);
    assert!(app.editor.close_document(third, false).is_ok());
    assert_eq!(order(&app), [second, first]);
    let fourth = app
        .editor
        .open(&dir.path().join("fourth.txt"), Action::Load)?;
    assert_eq!(order(&app), [second, first, fourth]);
    assert_eq!(
        app.editor.documents.keys().copied().collect::<Vec<_>>(),
        order(&app)
    );
    assert_eq!(
        app.editor
            .documents
            .iter()
            .map(|(id, doc)| {
                assert_eq!(*id, doc.id());
                *id
            })
            .collect::<Vec<_>>(),
        order(&app)
    );
    let mutable_order: Vec<_> = app.editor.documents_mut().map(|doc| doc.id()).collect();
    assert_eq!(mutable_order, order(&app));
    assert!(app.editor.close_document(second, false).is_ok());
    assert!(app.editor.close_document(fourth, false).is_ok());
    for command in ["previous", "next", "start", "end"] {
        helpers::keys(&mut app, &format!(":buffer-move-{command}<ret>")).await?;
        assert_eq!(order(&app), [first]);
    }
    assert!(app.close().await.is_empty());
    Ok(())
}

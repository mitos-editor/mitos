use super::Fixture;
use editor_core::Transaction;
use view::editor::Action;

#[tokio::test(flavor = "multi_thread")]
async fn breakpoints_follow_edits_and_history_without_changing_identity() -> anyhow::Result<()> {
    for newline in ["\n", "\r\n"] {
        let mut f = Fixture::new(&format!("first{newline}marked{newline}last{newline}"))?;
        let doc = view::current_ref!(f.editor).1;
        let (id, path) = (doc.id(), doc.path().unwrap().to_owned());
        f.editor.toggle_breakpoints(path.clone(), &[1])?;
        let identity = f.editor.debugger().breakpoints(&path)[0].identity();
        let doc = f.editor.document_mut(id).unwrap();
        doc.commit_changes();
        let transaction = Transaction::change(
            doc.text(),
            [(0, 0, Some(format!("added{newline}").into()))].into_iter(),
        );
        doc.apply(transaction.changes())?;
        doc.commit_changes();
        // Read immediately, without applying editor callbacks.
        assert_eq!(f.editor.debugger().breakpoints(&path)[0].line, 2);
        assert_eq!(
            f.editor.debugger().breakpoints(&path)[0].identity(),
            identity
        );
        assert!(f.editor.document_mut(id).unwrap().undo());
        assert_eq!(f.editor.debugger().breakpoints(&path)[0].line, 1);
        assert!(f.editor.document_mut(id).unwrap().redo());
        assert_eq!(f.editor.debugger().breakpoints(&path)[0].line, 2);
        assert_eq!(
            f.editor.debugger().breakpoints(&path)[0].identity(),
            identity
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_document_edits_map_markers_and_collapsed_lines_keep_the_oldest_identity(
) -> anyhow::Result<()> {
    for newline in ["\n", "\r\n"] {
        let mut f = Fixture::new("focused\n")?;
        let path = view::current_ref!(f.editor)
            .1
            .path()
            .unwrap()
            .with_file_name("hidden.words");
        std::fs::write(&path, format!("first{newline}second{newline}last{newline}"))?;
        f.editor.toggle_breakpoints(path.clone(), &[0, 1])?;
        let identity = f.editor.debugger().breakpoints(&path)[0].identity();
        let id = f.editor.open(&path, Action::Load)?;
        let doc = f.editor.document_mut(id).unwrap();
        assert_eq!(doc.path(), Some(path.as_path()));
        let end = doc.text().line_to_char(1);
        let transaction = Transaction::change(doc.text(), [(0, end, None)].into_iter());
        doc.apply(transaction.changes())?;
        let markers = f.editor.debugger().breakpoints(&path);
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].line, 0);
        assert_eq!(markers[0].identity(), identity);
        assert!(f.editor.document(id).unwrap().selections().next().is_none());
    }
    Ok(())
}

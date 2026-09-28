use editor_core::{Selection, Transaction};
use view::{current, current_ref, editor::Action, Editor};

use super::Fixture;

fn insert_snippet(editor: &mut Editor) -> anyhow::Result<()> {
    let snippet = snippets::Snippet::parse("${1:one} ${2:two}$0")?;
    let mut ctx = snippets::SnippetRenderCtx {
        resolve_var: Box::new(|_| None),
        tab_width: 4,
        indent_style: editor_core::indent::IndentStyle::Spaces(4),
        line_ending: "\n",
    };
    let (view, doc) = current!(editor);
    let (transaction, selection, rendered) = snippet.render(
        doc.text(),
        &Selection::point(0),
        |range| (range.from(), range.to()),
        &mut ctx,
    );
    assert!(doc.apply(&transaction, view.id));
    doc.set_selection(view.id, selection);
    doc.active_snippet = snippets::ActiveSnippet::new(rendered);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn snippet_lifecycle_is_installed_once_without_terminal_hooks() -> anyhow::Result<()> {
    let mut first = Fixture::new("\n\n")?;
    let _second = Fixture::new("\n")?;
    insert_snippet(&mut first.editor)?;
    let (view, doc) = current!(first.editor);
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("xx".into()))].into_iter())
        .with_selection(Selection::point(4));
    assert!(doc.apply(&transaction, view.id));
    assert_eq!(
        doc.active_snippet
            .as_ref()
            .unwrap()
            .tabstops()
            .next()
            .unwrap()
            .ranges[0]
            .start,
        2
    );
    doc.set_selection(view.id, Selection::point(doc.text().len_chars() - 1));
    assert!(doc.active_snippet.is_none());
    insert_snippet(&mut first.editor)?;
    let (view, doc) = current!(first.editor);
    let transaction =
        Transaction::delete(doc.text(), [(0, doc.text().len_chars() - 1)].into_iter());
    assert!(doc.apply(&transaction, view.id));
    assert!(doc.active_snippet.is_none());
    insert_snippet(&mut first.editor)?;
    let id = current_ref!(first.editor).1.id();
    first.editor.switch(id, Action::VerticalSplit);
    assert!(current_ref!(first.editor).1.active_snippet.is_none());
    Ok(())
}

use super::Fixture;
use editor_core::{Range, Selection};
use std::time::Duration;
use view::{current, current_ref, document::Mode, features::completion};

fn catalog() -> snippets::Catalog {
    snippets::Catalog::parse(
        r#"
        [[snippet]]
        name = "Greeting"
        prefix = ["hello", "hi"]
        body = '${1:world}:$TM_FILENAME:$CURSOR_NUMBER$0'
        [[snippet]]
        name = "Rust only"
        prefix = "rs"
        scope = "rust"
        body = 'fn ${1:name}() {$0}'
    "#,
    )
    .unwrap()
}

async fn request(
    f: &mut Fixture,
) -> anyhow::Result<(
    Vec<completion::CompletionItem>,
    completion::CompletionSession,
)> {
    f.editor.set_mode(Mode::Insert);
    let (view, doc) = current_ref!(f.editor);
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    f.editor
        .handlers()
        .trigger_completions(cursor, doc.id(), view.id);
    Ok(tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let callback = f.callbacks.recv().await.unwrap();
            callback(&mut f.editor);
            while let Some(update) = completion::next_update(&mut f.editor) {
                if let Some(completion::CompletionChange::Show { items, session }) =
                    update.apply(&mut f.editor)
                {
                    return (items, session);
                }
            }
        }
    })
    .await?)
}

#[tokio::test(flavor = "multi_thread")]
async fn native_snippets_complete_preview_accept_and_undo_without_lsp() -> anyhow::Result<()> {
    let mut f = Fixture::new("he\n")?;
    f.editor.set_snippet_catalog(catalog());
    let (view, doc) = current!(f.editor);
    doc.set_selection(view.id, Selection::point(2));
    let (items, session) = request(&mut f).await?;
    assert_eq!(
        items
            .iter()
            .filter(|item| matches!(item, completion::CompletionItem::Snippet(_)))
            .count(),
        2
    );
    assert!(!items.iter().any(|item| item.filter_text() == "rs"));
    let item = items
        .iter()
        .find(|item| item.filter_text() == "hello")
        .unwrap();
    assert!(f.editor.begin_completion(&session));
    assert!(f.editor.preview_completion(&session, item)?);
    let doc = current_ref!(f.editor).1;
    assert_eq!(doc.text(), "he\n");
    assert_eq!(
        f.editor.completion_presentation(doc).text(),
        "world:document.words:1\n"
    );
    f.editor.finish_completion();
    assert_eq!(current_ref!(f.editor).1.text(), "he\n");
    let (items, session) = request(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "hello")
        .unwrap();
    assert!(f.editor.accept_completion(&session, item)?);
    f.editor.finish_completion();
    f.editor.insert_char('界')?;
    assert_eq!(current_ref!(f.editor).1.text(), "界:document.words:1\n");
    let (view, doc) = current!(f.editor);
    assert!(view::editing::undo(doc, view.id));
    assert_eq!(doc.text(), "he\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn replacement_catalog_and_disabled_provider_reject_old_results() -> anyhow::Result<()> {
    let mut f = Fixture::new("he\n")?;
    f.editor.set_snippet_catalog(catalog());
    let (view, doc) = current!(f.editor);
    doc.set_selection(view.id, Selection::point(2));
    let (items, session) = request(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "hello")
        .unwrap();
    f.editor.set_snippet_catalog(snippets::Catalog::default());
    assert!(!f.editor.accept_completion(&session, item)?);
    f.editor.set_snippet_catalog(catalog());
    let (items, session) = request(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "hello")
        .unwrap();
    f.configure(|config| config.completion.snippets = false);
    assert!(!f.editor.preview_completion(&session, item)?);
    assert_eq!(current_ref!(f.editor).1.text(), "he\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn native_snippets_keep_multiple_cursors_and_variable_contexts() -> anyhow::Result<()> {
    let mut f = Fixture::new("he\nhe\n")?;
    f.editor.set_snippet_catalog(catalog());
    let (view, doc) = current!(f.editor);
    doc.set_selection(
        view.id,
        Selection::new(vec![Range::point(2), Range::point(5)].into(), 1),
    );
    let (items, session) = request(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "hello")
        .unwrap();
    assert!(f.editor.accept_completion(&session, item)?);
    f.editor.finish_completion();
    f.editor.insert_char('x')?;
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.text(), "x:document.words:1\nx:document.words:2\n");
    assert_eq!(doc.selection(view.id).primary_index(), 1);
    Ok(())
}

use editor_core::movement::Direction;
use editor_core::{Selection, Transaction};
use view::editor::SnippetNavigation;
use view::{current, current_ref, editor::Action, Editor};

use super::Fixture;

pub(super) fn insert_snippet(editor: &mut Editor) -> anyhow::Result<()> {
    editor.insert_snippet(&snippets::Snippet::parse("${1:one} ${2:two}$0")?)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn backward_navigation_at_the_first_tabstop_preserves_the_session() -> anyhow::Result<()> {
    let mut f = Fixture::new("\n")?;
    insert_snippet(&mut f.editor)?;
    let before = current_ref!(f.editor)
        .1
        .selection(current_ref!(f.editor).0.id)
        .clone();
    assert_eq!(
        f.editor.move_snippet_tabstop(Direction::Backward),
        SnippetNavigation::AtStart
    );
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.selection(view.id), &before);
    assert!(doc.active_snippet().is_some());
    assert_eq!(
        f.editor.move_snippet_tabstop(Direction::Forward),
        SnippetNavigation::Moved
    );
    f.editor.insert_char('界')?;
    f.editor.insert_char('é')?;
    assert_eq!(current_ref!(f.editor).1.text(), "one 界é\n");
    assert_eq!(
        f.editor.move_snippet_tabstop(Direction::Backward),
        SnippetNavigation::Moved
    );
    f.editor.insert_char('x')?;
    assert_eq!(current_ref!(f.editor).1.text(), "x 界é\n");
    assert_eq!(
        f.editor.move_snippet_tabstop(Direction::Forward),
        SnippetNavigation::Moved
    );
    assert_eq!(
        f.editor.move_snippet_tabstop(Direction::Forward),
        SnippetNavigation::Finished
    );
    assert_eq!(
        f.editor.move_snippet_tabstop(Direction::Backward),
        SnippetNavigation::Inactive
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_replaces_each_placeholder_once_and_shares_the_insert_undo_group(
) -> anyhow::Result<()> {
    for ending in ["\n", "\r\n"] {
        let mut f = Fixture::new(ending)?;
        insert_snippet(&mut f.editor)?;
        f.editor.insert_char('界')?;
        f.editor.insert_char('é')?;
        assert_eq!(
            current_ref!(f.editor).1.text().to_string(),
            format!("界é two{ending}")
        );
        let (view, doc) = current!(f.editor);
        assert!(view::editing::undo(doc, view.id));
        assert_eq!(doc.text().to_string(), ending);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn moving_inside_a_placeholder_keeps_its_text_when_typing() -> anyhow::Result<()> {
    let mut f = Fixture::new("\n")?;
    insert_snippet(&mut f.editor)?;
    let (view, doc) = current!(f.editor);
    doc.set_selection(view.id, Selection::point(1));
    f.editor.insert_char('x')?;
    assert_eq!(current_ref!(f.editor).1.text(), "oxne two\n");
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
    view::editing::apply(doc, &transaction, view.id).unwrap();
    assert_eq!(
        doc.active_snippet()
            .unwrap()
            .tabstops()
            .next()
            .unwrap()
            .ranges[0]
            .start,
        2
    );
    doc.set_selection(view.id, Selection::point(doc.text().len_chars() - 1));
    assert!(doc.active_snippet().is_none());
    insert_snippet(&mut first.editor)?;
    let (view, doc) = current!(first.editor);
    let transaction =
        Transaction::delete(doc.text(), [(0, doc.text().len_chars() - 1)].into_iter());
    view::editing::apply(doc, &transaction, view.id).unwrap();
    assert!(doc.active_snippet().is_none());
    insert_snippet(&mut first.editor)?;
    let id = current_ref!(first.editor).1.id();
    first.editor.switch(id, Action::VerticalSplit);
    assert!(current_ref!(first.editor).1.active_snippet().is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn document_switch_invalidates_the_previous_documents_snippet() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("\n")?;
    insert_snippet(&mut fixture.editor)?;
    let previous = current_ref!(fixture.editor).1.id();
    fixture.editor.new_file(Action::Replace);
    assert!(fixture
        .editor
        .document(previous)
        .unwrap()
        .active_snippet()
        .is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn history_replay_ends_the_active_snippet_session() -> anyhow::Result<()> {
    let mut f = Fixture::new("\n")?;
    insert_snippet(&mut f.editor)?;
    let (view, doc) = current!(f.editor);
    assert!(doc.active_snippet().is_some());
    assert!(view::editing::undo(doc, view.id));
    assert_eq!(doc.text().to_string(), "\n");
    assert!(doc.active_snippet().is_none());
    assert!(view::editing::redo(doc, view.id));
    assert_eq!(doc.text().to_string(), "one two\n");
    assert!(doc.active_snippet().is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn non_character_input_ends_placeholder_replacement() -> anyhow::Result<()> {
    let mut f = Fixture::new("\n")?;
    insert_snippet(&mut f.editor)?;
    f.editor.insert_tab(1)?;
    let before = current_ref!(f.editor).1.text().to_string();
    f.editor.insert_char('x')?;
    let text = current_ref!(f.editor).1.text().to_string();
    assert_eq!(text.replace('x', ""), before);
    assert_eq!(text.chars().count(), before.chars().count() + 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn replacing_the_only_placeholder_keeps_the_final_tabstop_navigable() -> anyhow::Result<()> {
    let mut f = Fixture::new("\r\n")?;
    f.editor
        .insert_snippet(&snippets::Snippet::parse("${1:one}$0")?)?;
    f.editor.insert_char('界')?;
    assert_eq!(current_ref!(f.editor).1.text(), "界\r\n");
    assert!(current_ref!(f.editor).1.active_snippet().is_some());
    assert_eq!(
        f.editor.move_snippet_tabstop(Direction::Forward),
        SnippetNavigation::Finished
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_pairs_replace_placeholders_at_every_cursor() -> anyhow::Result<()> {
    let mut f = Fixture::new("\n\n")?;
    let (view, doc) = current!(f.editor);
    doc.set_selection(
        view.id,
        Selection::new(
            vec![editor_core::Range::point(0), editor_core::Range::point(1)].into(),
            0,
        ),
    );
    insert_snippet(&mut f.editor)?;
    f.editor.insert_char('(')?;
    assert_eq!(current_ref!(f.editor).1.text(), "() two\n() two\n");
    assert_eq!(
        f.editor.move_snippet_tabstop(Direction::Forward),
        SnippetNavigation::Moved
    );
    f.editor.insert_char('界')?;
    assert_eq!(current_ref!(f.editor).1.text(), "() 界\n() 界\n");
    Ok(())
}

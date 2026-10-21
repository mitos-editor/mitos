use super::Fixture;
use editor_core::{
    movement::{Direction, Movement},
    search::SearchMatchLimit,
    Range, Selection,
};
use stdx::rope;
use view::{
    current, current_ref,
    editor::{Action, SearchOutcome},
};

#[tokio::test(flavor = "multi_thread")]
async fn shared_search_counts_matches_and_preserves_other_splits() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("one one one\n")?;
    let doc_id = current_ref!(fixture.editor).1.id();
    fixture.editor.switch(doc_id, Action::VerticalSplit);
    let other = fixture.editor.tree.focus();
    fixture.editor.focus_prev();
    let regex = rope::Regex::new("one")?;
    assert_eq!(
        fixture
            .editor
            .search_buffer(&regex, Movement::Move, Direction::Forward, 2, false),
        SearchOutcome::Found { wrapped: false }
    );
    let (view, doc) = current_ref!(fixture.editor);
    assert_eq!(doc.selection(view.id).primary(), Range::new(8, 11));
    assert_eq!(doc.selection(other), &Selection::single(0, 1));
    let search = doc.get_last_search_match(view.id).unwrap();
    assert_eq!(search.idx, 3);
    assert!(matches!(search.count, SearchMatchLimit::Limitless(3)));
    assert_eq!(
        fixture
            .editor
            .search_buffer(&regex, Movement::Move, Direction::Forward, 1, false),
        SearchOutcome::NotFound
    );
    assert_eq!(
        fixture
            .editor
            .search_buffer(&regex, Movement::Move, Direction::Forward, 1, true),
        SearchOutcome::Found { wrapped: true }
    );
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(view.id, Selection::single(0, 1));
    assert_eq!(
        fixture
            .editor
            .search_buffer(&regex, Movement::Extend, Direction::Forward, 2, false),
        SearchOutcome::Found { wrapped: false }
    );
    let (view, doc) = current_ref!(fixture.editor);
    assert_eq!(doc.selection(view.id).len(), 3);
    assert_eq!(doc.selection(view.id).primary(), Range::new(8, 11));
    assert!(doc.changes().is_empty());
    Ok(())
}

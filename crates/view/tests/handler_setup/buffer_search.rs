use super::Fixture;
use editor_core::{
    movement::{Direction, Movement},
    Range, Selection,
};
use view::features::buffer_search::Query;
use view::{
    current, current_ref,
    editor::{Action, SearchOutcome},
};

use editor_config::{OptionToml, StatusLineElement};
use editor_core::{
    search::statistics::{Count, Position},
    Transaction,
};
use tokio::sync::mpsc::UnboundedReceiver;
use view::{callbacks::EditorCallback, features::buffer_search::Handler};

fn counting_fixture(text: &str) -> anyhow::Result<(Fixture, UnboundedReceiver<EditorCallback>)> {
    let (sender, updates) = super::support::callback_channel();
    let fixture = Fixture::with_handler_setup(
        text,
        "language = []",
        loader::syntax::Resources::default(),
        |config| {
            config
                .statusline
                .right
                .push(StatusLineElement::SearchPosition)
        },
        |handlers, _| handlers.buffer_search = Handler::new(sender),
    )?;
    Ok((fixture, updates))
}

async fn next_update(updates: &mut UnboundedReceiver<EditorCallback>) -> EditorCallback {
    tokio::time::timeout(std::time::Duration::from_secs(5), updates.recv())
        .await
        .unwrap()
        .unwrap()
}

fn position(fixture: &Fixture) -> Option<Position> {
    let (view, doc) = current_ref!(fixture.editor);
    doc.search_position(view.id)
}

fn search(fixture: &mut Fixture, pattern: &str, count: usize) -> anyhow::Result<SearchOutcome> {
    Ok(fixture.editor.search_buffer(
        &Query::new(pattern, true, false)?,
        Movement::Move,
        Direction::Forward,
        count,
        true,
    ))
}

#[tokio::test(flavor = "multi_thread")]
async fn statistics_are_optional_and_reused_across_recompiled_queries_and_counted_navigation(
) -> anyhow::Result<()> {
    let mut disabled = Fixture::new("one one one\n")?;
    search(&mut disabled, "one", 1)?;
    assert_eq!(position(&disabled), None);

    let (mut fixture, mut updates) = counting_fixture("one one one\n")?;
    search(&mut fixture, "one", 1)?;
    assert_eq!(position(&fixture), Some(Position::PENDING));
    let update = next_update(&mut updates).await;
    // A queued scan belongs to the query, not the cursor position at submission.
    search(&mut fixture, "one", 1)?;
    update(&mut fixture.editor);
    assert_eq!(
        position(&fixture),
        Some(Position {
            current: Count::Exact(3),
            total: Count::Exact(3)
        })
    );
    search(&mut fixture, "one", 100)?;
    assert_eq!(
        position(&fixture),
        Some(Position {
            current: Count::Exact(1),
            total: Count::Exact(3)
        })
    );
    assert!(updates.try_recv().is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cached_and_queued_statistics_are_invalidated_by_document_only_edits() -> anyhow::Result<()>
{
    for queued in [true, false] {
        let (mut fixture, mut updates) = counting_fixture("one one one\n")?;
        search(&mut fixture, "one", 2)?;
        let update = next_update(&mut updates).await;
        let held = if queued {
            Some(update)
        } else {
            update(&mut fixture.editor);
            None
        };
        let (_, doc) = current!(fixture.editor);
        let edit = Transaction::change(doc.text(), [(0, 3, Some("two".into()))].into_iter());
        doc.apply(edit.changes())?;
        assert_eq!(position(&fixture), None);
        if let Some(update) = held {
            update(&mut fixture.editor);
        }
        assert_eq!(position(&fixture), None);
        search(&mut fixture, "one", 1)?;
        next_update(&mut updates).await(&mut fixture.editor);
        assert_eq!(position(&fixture).unwrap().total, Count::Exact(2));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn split_edits_and_undo_invalidate_all_views_even_when_text_becomes_identical(
) -> anyhow::Result<()> {
    let (mut fixture, mut updates) = counting_fixture("one one one\n")?;
    let (view, doc) = current_ref!(fixture.editor);
    let (first, id) = (view.id, doc.id());
    search(&mut fixture, "one", 1)?;
    let first_update = next_update(&mut updates).await;
    fixture.editor.switch(id, Action::VerticalSplit);
    search(&mut fixture, "one", 1)?;
    let second_update = next_update(&mut updates).await;
    let (view, doc) = current!(fixture.editor);
    let second = view.id;
    doc.commit_changes();
    let transaction = Transaction::change(doc.text(), [(0, 3, Some("two".into()))].into_iter())
        .with_selection(Selection::single(0, 3));
    view::editing::apply(doc, &transaction, second)?;
    doc.commit_changes();
    assert_eq!(doc.search_position(first), None);
    assert_eq!(doc.search_position(second), None);
    assert!(view::editing::undo(doc, second));
    assert_eq!(doc.text().to_string(), "one one one\n");
    first_update(&mut fixture.editor);
    second_update(&mut fixture.editor);
    let doc = fixture.editor.document(id).unwrap();
    assert_eq!(doc.search_position(first), None);
    assert_eq!(doc.search_position(second), None);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_statistics_cannot_replace_a_new_query_or_follow_a_switched_view(
) -> anyhow::Result<()> {
    let (mut fixture, mut updates) = counting_fixture("one two one two\n")?;
    search(&mut fixture, "one", 1)?;
    let old = next_update(&mut updates).await;
    search(&mut fixture, "two", 1)?;
    old(&mut fixture.editor);
    assert_eq!(position(&fixture), Some(Position::PENDING));
    next_update(&mut updates).await(&mut fixture.editor);
    assert_eq!(position(&fixture).unwrap().total, Count::Exact(2));

    search(&mut fixture, "o", 1)?;
    let old = next_update(&mut updates).await;
    let id = current_ref!(fixture.editor).1.id();
    fixture.editor.new_file(Action::Replace);
    fixture.editor.switch(id, Action::Replace);
    old(&mut fixture.editor);
    assert_eq!(position(&fixture), None);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn selection_changes_clear_display_without_throwing_away_a_reusable_scan(
) -> anyhow::Result<()> {
    let (mut fixture, mut updates) = counting_fixture("one one one\n")?;
    search(&mut fixture, "one", 1)?;
    let old = next_update(&mut updates).await;
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(view.id, Selection::single(0, 1));
    old(&mut fixture.editor);
    assert_eq!(position(&fixture), None);
    search(&mut fixture, "one", 1)?;
    assert_eq!(
        position(&fixture),
        Some(Position {
            current: Count::Exact(2),
            total: Count::Exact(3)
        })
    );
    assert!(updates.try_recv().is_err());
    search(&mut fixture, "missing", 1)?;
    assert_eq!(position(&fixture), None);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn splits_own_independent_queries_and_closure_rejects_queued_results() -> anyhow::Result<()> {
    let (mut fixture, mut updates) = counting_fixture("one two one two\n")?;
    let (view, doc) = current_ref!(fixture.editor);
    let (first, id) = (view.id, doc.id());
    search(&mut fixture, "one", 1)?;
    let first_update = next_update(&mut updates).await;
    fixture.editor.switch(id, Action::VerticalSplit);
    let second = current_ref!(fixture.editor).0.id;
    search(&mut fixture, "two", 1)?;
    let second_update = next_update(&mut updates).await;
    first_update(&mut fixture.editor);
    assert!(fixture
        .editor
        .document(id)
        .unwrap()
        .search_position(first)
        .is_some());
    fixture.editor.close(second);
    second_update(&mut fixture.editor);
    let doc = fixture.editor.document(id).unwrap();
    assert_eq!(doc.search_position(second), None);
    assert_eq!(doc.search_position(first).unwrap().total, Count::Exact(2));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn configuration_changes_cancel_counts_and_apply_new_limits_and_case_options(
) -> anyhow::Result<()> {
    let (mut fixture, mut updates) = counting_fixture("One one one\n")?;
    search(&mut fixture, "one", 1)?;
    let old = next_update(&mut updates).await;
    fixture.configure(|config| config.statusline.right.clear());
    old(&mut fixture.editor);
    assert_eq!(position(&fixture), None);
    fixture.configure(|config| {
        config
            .statusline
            .right
            .push(StatusLineElement::SearchPosition);
        config.search.max_matches = OptionToml::Some(1);
    });
    search(&mut fixture, "one", 1)?;
    next_update(&mut updates).await(&mut fixture.editor);
    assert_eq!(position(&fixture).unwrap().total, Count::MoreThan(1));
    fixture.configure(|config| {
        config.search.max_matches = OptionToml::None;
        config.search.smart_case = false;
    });
    assert_eq!(position(&fixture), None);
    let query = Query::new("one", false, false)?;
    fixture
        .editor
        .search_buffer(&query, Movement::Move, Direction::Forward, 1, true);
    next_update(&mut updates).await(&mut fixture.editor);
    assert_eq!(position(&fixture).unwrap().total, Count::Exact(2));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_search_counts_matches_and_preserves_other_splits() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("one one one\n")?;
    let doc_id = current_ref!(fixture.editor).1.id();
    fixture.editor.switch(doc_id, Action::VerticalSplit);
    let other = fixture.editor.tree.focus();
    fixture.editor.focus_prev();
    let query = Query::new("one", true, false)?;
    assert_eq!(
        fixture
            .editor
            .search_buffer(&query, Movement::Move, Direction::Forward, 2, false),
        SearchOutcome::Found { wrapped: false }
    );
    let (view, doc) = current_ref!(fixture.editor);
    assert_eq!(doc.selection(view.id).primary(), Range::new(8, 11));
    assert_eq!(doc.selection(other), &Selection::single(0, 1));
    assert_eq!(
        fixture
            .editor
            .search_buffer(&query, Movement::Move, Direction::Forward, 1, false),
        SearchOutcome::NotFound
    );
    assert_eq!(
        fixture
            .editor
            .search_buffer(&query, Movement::Move, Direction::Forward, 1, true),
        SearchOutcome::Found { wrapped: true }
    );
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(view.id, Selection::single(0, 1));
    assert_eq!(
        fixture
            .editor
            .search_buffer(&query, Movement::Extend, Direction::Forward, 2, false),
        SearchOutcome::Found { wrapped: false }
    );
    let (view, doc) = current_ref!(fixture.editor);
    assert_eq!(doc.selection(view.id).len(), 3);
    assert_eq!(doc.selection(view.id).primary(), Range::new(8, 11));
    assert!(doc.changes().is_empty());
    Ok(())
}

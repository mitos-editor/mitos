use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use editor_core::{Selection, Transaction};
use event::register_hook;
use view::{
    current,
    events::{DocumentDidChange, SelectionDidChange},
};

use super::{snippets::insert_snippet, Fixture};

#[tokio::test(flavor = "multi_thread")]
async fn document_observers_see_mapped_snippets_and_the_transaction_selection() -> anyhow::Result<()>
{
    // Install the observer before editor services to exercise the public event contract
    // independently of built-in observer registration order.
    event::register_event::<DocumentDidChange>();
    let armed = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let observe = armed.clone();
    let observed = calls.clone();
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if observe.load(Ordering::Relaxed) {
            let view = event.view.expect("native edit has an originating view");
            assert_eq!(event.doc.text().to_string(), "xxone two\n\n");
            assert_eq!(event.doc.selection(view), &Selection::single(4, 5));
            assert_eq!(
                event
                    .doc
                    .active_snippet
                    .as_ref()
                    .unwrap()
                    .tabstops()
                    .next()
                    .unwrap()
                    .ranges[0]
                    .start,
                2
            );
            observed.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    });
    let mut fixture = Fixture::new("\n\n")?;
    insert_snippet(&mut fixture.editor)?;
    armed.store(true, Ordering::Relaxed);
    let (view, doc) = current!(fixture.editor);
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("xx".into()))].into_iter())
        .with_selection(Selection::point(4));
    assert!(view::editing::apply(doc, &transaction, view.id));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn selection_observers_see_invalid_snippets_removed() -> anyhow::Result<()> {
    event::register_event::<SelectionDidChange>();
    let armed = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let observe = armed.clone();
    let observed = calls.clone();
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        if observe.load(Ordering::Relaxed) {
            assert!(event.doc.active_snippet.is_none());
            observed.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    });
    let mut fixture = Fixture::new("\n\n")?;
    insert_snippet(&mut fixture.editor)?;
    armed.store(true, Ordering::Relaxed);
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(view.id, Selection::point(doc.text().len_chars() - 1));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn document_observers_can_commit_the_complete_undo_group() -> anyhow::Result<()> {
    event::register_event::<DocumentDidChange>();
    let armed = Arc::new(AtomicBool::new(false));
    let observe = armed.clone();
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if observe.load(Ordering::Relaxed) {
            assert!(!event.doc.changes().is_empty());
            assert!(event.doc.commit_changes().is_some());
        }
        Ok(())
    });
    let mut fixture = Fixture::new("abc\n")?;
    let (_, doc) = current!(fixture.editor);
    let transaction = Transaction::change(doc.text(), [(0, 1, Some("界".into()))].into_iter());
    armed.store(true, Ordering::Relaxed);
    doc.apply(transaction.changes())?;
    armed.store(false, Ordering::Relaxed);
    assert!(doc.changes().is_empty());
    assert!(doc.undo());
    assert_eq!(doc.text(), "abc\n");
    assert!(doc.redo());
    assert_eq!(doc.text(), "界bc\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn history_observers_see_the_revision_of_each_intermediate_text() -> anyhow::Result<()> {
    use buffer::history::UndoKind;
    let mut fixture = Fixture::new("abc\n")?;
    let (_, doc) = current!(fixture.editor);
    for (from, to, text) in [(0, 1, "A"), (1, 2, "B")] {
        let transaction =
            Transaction::change(doc.text(), [(from, to, Some(text.into()))].into_iter());
        doc.apply(transaction.changes())?;
        doc.commit_changes();
    }
    assert!(doc.undo());
    let transaction = Transaction::change(doc.text(), [(1, 2, Some("Z".into()))].into_iter());
    doc.apply(transaction.changes())?;
    doc.commit_changes();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let records = observed.clone();
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        records.lock().unwrap().push((
            event.doc.snapshot().revision(),
            event.doc.text().to_string(),
        ));
        Ok(())
    });
    assert!(doc.earlier(UndoKind::Steps(1)));
    assert_eq!(
        *observed.lock().unwrap(),
        [(1, "Abc\n".into()), (2, "ABc\n".into())]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_from_history_observers_stop_remaining_replay_steps() -> anyhow::Result<()> {
    use buffer::history::UndoKind;
    let mut fixture = Fixture::new("abc\n")?;
    let (_, doc) = current!(fixture.editor);
    for (from, to, text) in [(0, 1, "A"), (1, 2, "B")] {
        let transaction =
            Transaction::change(doc.text(), [(from, to, Some(text.into()))].into_iter());
        doc.apply(transaction.changes())?;
        doc.commit_changes();
    }
    assert!(doc.undo());
    let transaction = Transaction::change(doc.text(), [(1, 2, Some("Z".into()))].into_iter());
    doc.apply(transaction.changes())?;
    doc.commit_changes();
    let armed = Arc::new(AtomicBool::new(true));
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if armed.swap(false, Ordering::Relaxed) {
            let transaction =
                Transaction::change(event.doc.text(), [(0, 1, Some("Q".into()))].into_iter());
            event.doc.apply(transaction.changes())?;
            event.doc.commit_changes();
        }
        Ok(())
    });
    assert!(doc.earlier(UndoKind::Steps(1)));
    assert_eq!(doc.text(), "Qbc\n");
    assert!(doc.undo());
    assert_eq!(doc.text(), "Abc\n");
    Ok(())
}

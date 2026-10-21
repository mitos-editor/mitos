use editor_core::Selection;
use std::time::Duration;
use view::{current, current_ref, document::Mode, features::completion};

use super::Fixture;

#[tokio::test(flavor = "multi_thread")]
async fn headless_setup_runs_completion_through_its_callback_destination() -> anyhow::Result<()> {
    let mut f = Fixture::new("alpha alphabet al\n")?;
    let (items, _, _) = request_words(&mut f).await?;
    assert!(items.iter().any(|item| item.filter_text() == "alpha"));
    assert!(items.iter().any(|item| item.filter_text() == "alphabet"));
    Ok(())
}

async fn request_words(
    f: &mut Fixture,
) -> anyhow::Result<(
    Vec<completion::CompletionItem>,
    completion::CompletionSession,
    usize,
)> {
    f.expect_words(&["alpha", "alphabet"]).await?;
    let (view, doc) = current!(f.editor);
    let cursor = doc.text().len_chars() - 1;
    doc.set_selection(view.id, Selection::point(cursor));
    f.editor.set_mode(Mode::Insert);
    let (view, doc) = current_ref!(f.editor);
    f.editor
        .handlers()
        .trigger_completions(cursor, doc.id(), view.id);
    let (items, session) = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let callback = f.callbacks.recv().await.expect("callback destination open");
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
    .await?;
    Ok((items, session, cursor))
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_cancellation_restores_typing_without_creating_an_undo_edit() -> anyhow::Result<()>
{
    let mut f = Fixture::new("alpha alphabet al\n")?;
    let (items, session, trigger) = request_words(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "alpha")
        .unwrap();
    let (view, doc) = current!(f.editor);
    let typing = editor_core::Transaction::change(
        doc.text(),
        [(trigger, trigger, Some("p".into()))].into_iter(),
    );
    view::editing::apply(doc, &typing, view.id).unwrap();
    assert!(f.editor.preview_completion(&session, item).unwrap());
    assert_eq!(
        current_ref!(f.editor).1.text().to_string(),
        "alpha alphabet alpha\n"
    );
    assert!(f.editor.finish_completion().unwrap().is_none());
    assert_eq!(
        current_ref!(f.editor).1.text().to_string(),
        "alpha alphabet alp\n"
    );
    let (view, doc) = current!(f.editor);
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "alpha alphabet al\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn acceptance_owns_preview_restoration_undo_and_repeat_metadata() -> anyhow::Result<()> {
    let mut f = Fixture::new("alpha alphabet al\n")?;
    let (items, session, trigger) = request_words(&mut f).await?;
    assert!(f.editor.begin_completion(&session));
    let alpha = items
        .iter()
        .find(|item| item.filter_text() == "alpha")
        .unwrap();
    let alphabet = items
        .iter()
        .find(|item| item.filter_text() == "alphabet")
        .unwrap();
    assert!(f.editor.preview_completion(&session, alpha).unwrap());
    assert!(f.editor.preview_completion(&session, alphabet).unwrap());
    assert!(!f.editor.begin_completion(&session));
    assert!(f.editor.accept_completion(&session, alphabet).unwrap());
    assert!(!f.editor.accept_completion(&session, alpha).unwrap());
    assert!(!f.editor.begin_completion(&session));
    assert_eq!(
        current_ref!(f.editor).1.text().to_string(),
        "alpha alphabet alphabet\n"
    );
    let accepted = f.editor.finish_completion().unwrap().unwrap();
    assert_eq!(accepted.trigger_offset, trigger);
    assert_eq!(accepted.changes, vec![(15, 17, Some("alphabet".into()))]);
    assert!(current_ref!(f.editor).1.active_snippet().is_none());
    assert!(f.editor.finish_completion().unwrap().is_none());
    let (view, doc) = current!(f.editor);
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "alpha alphabet al\n");
    assert!(view::editing::redo(doc, view));
    assert_eq!(doc.text().to_string(), "alpha alphabet alphabet\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_completion_items_cannot_preview_or_edit_another_document() -> anyhow::Result<()> {
    let mut f = Fixture::new("alpha alphabet al\n")?;
    let (items, session, _) = request_words(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "alpha")
        .unwrap();
    f.editor.new_file(view::editor::Action::Replace);
    let before = current_ref!(f.editor).1.text().clone();
    assert!(!f.editor.begin_completion(&session));
    assert!(!f.editor.preview_completion(&session, item).unwrap());
    assert!(!f.editor.accept_completion(&session, item).unwrap());
    assert!(f.editor.finish_completion().unwrap().is_none());
    assert_eq!(current_ref!(f.editor).1.text(), &before);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn replaced_sessions_reject_old_items_from_the_same_provider() -> anyhow::Result<()> {
    let mut f = Fixture::new("alpha alphabet al\n")?;
    let (old_items, old_session, _) = request_words(&mut f).await?;
    let (items, session, _) = request_words(&mut f).await?;
    let old = old_items
        .iter()
        .find(|item| item.filter_text() == "alpha")
        .unwrap();
    assert!(!f.editor.preview_completion(&old_session, old).unwrap());
    assert!(!f.editor.accept_completion(&old_session, old).unwrap());
    assert_eq!(
        current_ref!(f.editor).1.text().to_string(),
        "alpha alphabet al\n"
    );
    let item = items
        .iter()
        .find(|item| item.filter_text() == "alphabet")
        .unwrap();
    assert!(f.editor.accept_completion(&session, item).unwrap());
    assert_eq!(
        current_ref!(f.editor).1.text().to_string(),
        "alpha alphabet alphabet\n"
    );
    assert!(f.editor.finish_completion().unwrap().is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_typing_starts_and_filters_completions_without_terminal_hooks() -> anyhow::Result<()>
{
    let mut f = Fixture::with_config(
        "alpha alphabet \n",
        "language = []",
        Default::default(),
        |config| {
            config.completion.auto_trigger = true;
            config.completion.trigger_length = 1;
            config.completion.word.trigger_length = std::num::NonZeroU8::new(1).unwrap();
            config.completion.trigger_delay = Duration::ZERO;
        },
    )?;
    f.expect_words(&["alpha", "alphabet"]).await?;
    let (view, doc) = current!(f.editor);
    doc.set_selection(view.id, Selection::point(doc.text().len_chars() - 1));
    f.editor.set_mode(Mode::Insert);
    f.editor.insert_char('a')?;
    let (items, session) = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let callback = f
                .callbacks
                .recv()
                .await
                .expect("editor callback destination");
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
    .await?;
    assert!(items.iter().any(|item| item.filter_text() == "alpha"));
    assert!(f.editor.begin_completion(&session));
    f.editor.insert_char('l')?;
    let update = completion::next_update(&mut f.editor).expect("input filter update");
    assert!(matches!(
        update.apply(&mut f.editor),
        Some(completion::CompletionChange::Filter {
            character: Some('l')
        })
    ));
    f.editor.delete_char_backward(1)?;
    let update = completion::next_update(&mut f.editor).expect("backspace filter update");
    assert!(matches!(
        update.apply(&mut f.editor),
        Some(completion::CompletionChange::Filter { character: None })
    ));
    f.editor.insert_char('l')?;
    let stale = completion::next_update(&mut f.editor).expect("queued input filter update");
    f.editor.new_file(view::editor::Action::Replace);
    assert!(stale.apply(&mut f.editor).is_none());
    Ok(())
}

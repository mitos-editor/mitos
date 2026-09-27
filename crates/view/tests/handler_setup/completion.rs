use editor_core::Selection;
use std::time::Duration;
use view::{current, current_ref, document::Mode, handlers::completion};

use super::Fixture;

#[tokio::test(flavor = "multi_thread")]
async fn headless_setup_runs_completion_through_its_callback_destination() -> anyhow::Result<()> {
    let mut f = Fixture::new("alpha alphabet al\n")?;
    f.expect_words(&["alpha", "alphabet"]).await?;
    let (view, doc) = current!(f.editor);
    let cursor = doc.text().len_chars() - 1;
    doc.set_selection(view.id, Selection::point(cursor));
    f.editor.mode = Mode::Insert;
    let (view, doc) = current_ref!(f.editor);
    f.editor
        .handlers()
        .trigger_completions(cursor, doc.id(), view.id);
    let items = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let callback = f.callbacks.recv().await.expect("callback destination open");
            callback(&mut f.editor);
            while let Some(update) = completion::next_update(&mut f.editor) {
                if let Some(completion::CompletionChange::Show { items, .. }) =
                    update.apply(&mut f.editor)
                {
                    return items;
                }
            }
        }
    })
    .await?;
    assert!(items.iter().any(|item| item.filter_text() == "alpha"));
    assert!(items.iter().any(|item| item.filter_text() == "alphabet"));
    Ok(())
}

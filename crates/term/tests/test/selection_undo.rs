//! `undo_selection` (`Alt-k`) and `redo_selection` (`Alt-w`) step through selection
//! changes per view, and an edit starts a new history.
use view::{current_ref, editor::Action};

use super::helpers::{keys, test, test_config, AppBuilder};

#[tokio::test(flavor = "multi_thread")]
async fn alt_k_and_alt_w_step_through_selection_changes_with_counts() -> anyhow::Result<()> {
    test(("#[a|]#bc", "ll<A-k>", "a#[b|]#c")).await?;
    test(("#[a|]#bc", "ll<A-k><A-w>", "ab#[c|]#")).await?;
    test(("#[a|]#bc", "ll2<A-k>", "#[a|]#bc")).await?;
    // Counts past either end stop at the oldest or newest selection.
    test(("#[a|]#bc", "ll9<A-k>", "#[a|]#bc")).await?;
    test(("#[a|]#bc", "ll2<A-k>9<A-w>", "ab#[c|]#")).await?;
    // A new selection after undoing drops the undone ones.
    test(("#[a|]#bcd", "lll2<A-k>h<A-w>", "#[a|]#bcd")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn alt_k_restores_selections_replaced_by_a_mistyped_command() -> anyhow::Result<()> {
    // `,` keeps only the primary selection; `Alt-k` brings the others back.
    test((
        "#[one|]# two one",
        "%sone<ret>,<A-k>",
        "#(one|)# two #[one|]#",
    ))
    .await?;
    // `;` collapses each selection to its cursor.
    test((
        "#[one|]# two one",
        "%sone<ret>;<A-k>",
        "#(one|)# two #[one|]#",
    ))
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_start_a_new_selection_history() -> anyhow::Result<()> {
    // After `d`, the selections before the edit are gone from the history.
    test(("#[a|]#bcd", "lld<A-k>", "ab#[d|]#")).await?;
    // Text undo restores the edit's selection, and the history starts from there.
    test(("#[a|]#bcd", "lldu<A-k>", "ab#[c|]#d")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn each_split_keeps_its_own_history_and_reports_either_end() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "abc\n")?;
    let mut app = AppBuilder::new().with_config(test_config()).build()?;
    app.editor.open(&path, Action::Replace)?;
    let cursor = |app: &term::application::TestApplication| {
        let (view, doc) = current_ref!(app.editor);
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..))
    };
    let status = |app: &term::application::TestApplication| {
        app.editor
            .get_status()
            .map(|(message, _)| message.to_string())
            .unwrap_or_default()
    };

    keys(&mut app, "<A-k>").await?;
    assert_eq!(status(&app), "Already at oldest selection");
    keys(&mut app, "<A-w>").await?;
    assert_eq!(status(&app), "Already at newest selection");

    keys(&mut app, "ll").await?;
    // A new split starts without the first split's history.
    keys(&mut app, "<C-w>v<A-k>").await?;
    assert_eq!(cursor(&app), 2);
    assert_eq!(status(&app), "Already at oldest selection");
    keys(&mut app, "<C-w>w<A-k>").await?;
    assert_eq!(cursor(&app), 1);
    Ok(())
}

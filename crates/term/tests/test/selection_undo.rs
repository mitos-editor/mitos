//! `undo_selection` (`Alt-u`) and `redo_selection` (`Alt-Shift-u`) step through selection
//! changes per view, and an edit starts a new history.
use view::{current_ref, editor::Action};

use super::helpers::{keys, test, test_config, AppBuilder, LineFeedHandling};

#[tokio::test(flavor = "multi_thread")]
async fn alt_u_and_alt_shift_u_step_through_selection_changes_with_counts() -> anyhow::Result<()> {
    test(("#[a|]#bc", "ll<A-u>", "a#[b|]#c")).await?;
    test(("#[a|]#bc", "ll<A-u><A-U>", "ab#[c|]#")).await?;
    test(("#[a|]#bc", "ll2<A-u>", "#[a|]#bc")).await?;
    // Counts past either end stop at the oldest or newest selection.
    test(("#[a|]#bc", "ll9<A-u>", "#[a|]#bc")).await?;
    test(("#[a|]#bc", "ll2<A-u>9<A-U>", "ab#[c|]#")).await?;
    // A new selection after undoing drops the undone ones.
    test(("#[a|]#bcd", "lll2<A-u>h<A-U>", "#[a|]#bcd")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn alt_u_restores_selections_replaced_by_a_mistyped_command() -> anyhow::Result<()> {
    // `,` keeps only the primary selection; `Alt-u` brings the others back.
    test((
        "#[one|]# two one",
        "%sone<ret>,<A-u>",
        "#(one|)# two #[one|]#",
    ))
    .await?;
    // `;` collapses each selection to its cursor.
    test((
        "#[one|]# two one",
        "%sone<ret>;<A-u>",
        "#(one|)# two #[one|]#",
    ))
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_start_a_new_selection_history() -> anyhow::Result<()> {
    // After `d`, the selections before the edit are gone from the history.
    test(("#[a|]#bcd", "lld<A-u>", "ab#[d|]#")).await?;
    // Text undo restores the edit's selection, and the history starts from there.
    test(("#[a|]#bcd", "lldu<A-u>", "ab#[c|]#d")).await?;
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

    keys(&mut app, "<A-u>").await?;
    assert_eq!(status(&app), "Already at oldest selection");
    keys(&mut app, "<A-U>").await?;
    assert_eq!(status(&app), "Already at newest selection");

    keys(&mut app, "ll").await?;
    // A new split starts without the first split's history.
    keys(&mut app, "<C-w>v<A-u>").await?;
    assert_eq!(cursor(&app), 2);
    assert_eq!(status(&app), "Already at oldest selection");
    keys(&mut app, "<C-w>w<A-u>").await?;
    assert_eq!(cursor(&app), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn canceled_search_preserves_selection_history() -> anyhow::Result<()> {
    test(("#[a|]#bcd", "ll/d<esc><A-u>", "a#[b|]#cd")).await?;
    test(("#[a|]#bcd", "ll<A-u>/d<esc><A-U>", "ab#[c|]#d")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn confirmed_search_records_one_selection_change() -> anyhow::Result<()> {
    test(("#[a|]#bcd", "l/cd<ret>2<A-u>", "#[a|]#bcd")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn canceled_regex_selection_preserves_redo() -> anyhow::Result<()> {
    test((
        "#[one|]# two one",
        "%<A-u>sone<esc><A-U>",
        "#[one two one\n|]#",
    ))
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_or_invalid_search_preserves_redo() -> anyhow::Result<()> {
    for prompt in ["/d<backspace><ret>", "/[<ret>", "/z<ret>"] {
        test(("#[a|]#bcd", format!("ll<A-u>{prompt}<A-U>"), "ab#[c|]#d")).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn confirmed_regex_selection_records_one_change() -> anyhow::Result<()> {
    test(("#[one|]# two one", "%sone<ret>2<A-u>", "#[one|]# two one")).await?;
    test(("#[a|]#bc", "vl<A-u><A-U>", "#[ab|]#c")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn g_minus_and_g_plus_traverse_text_history() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_config(test_config()).build()?;
    keys(&mut app, "ione<esc>itwo<esc>g-").await?;
    assert_eq!(
        current_ref!(app.editor).1.text(),
        LineFeedHandling::Native.apply("one").as_str()
    );
    keys(&mut app, "g+").await?;
    assert_eq!(
        current_ref!(app.editor).1.text(),
        LineFeedHandling::Native.apply("onetwo").as_str()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn search_highlights_restore_on_cancel_and_clear_on_invalid_or_empty_input(
) -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[o|]#ne two one\n")
        .build()?;
    keys(&mut app, "/one<ret>").await?;
    let query_matches_one = |app: &term::application::TestApplication| {
        use stdx::rope::RopeSliceExt as _;
        let (view, doc) = current_ref!(app.editor);
        doc.search_highlight(view.id).is_some_and(|query| {
            query
                .regex()
                .is_match(editor_core::Rope::from("one").slice(..).regex_input())
        })
    };
    assert!(query_matches_one(&app));
    // Selection regex prompts do not own buffer search highlighting.
    keys(&mut app, "s<ret>").await?;
    assert!(query_matches_one(&app));
    keys(&mut app, "s[<ret>").await?;
    assert!(query_matches_one(&app));
    keys(&mut app, "/two<esc>").await?;
    assert!(query_matches_one(&app));
    keys(&mut app, "/[<ret>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert!(doc.search_highlight(view.id).is_none());
    keys(&mut app, "/one<ret>/two<backspace><backspace><backspace>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert!(doc.search_highlight(view.id).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn search_highlights_stay_cleared_after_a_background_edit_during_prompt() -> anyhow::Result<()>
{
    let mut app = AppBuilder::new()
        .with_input_text("#[o|]#ne two one\n")
        .build()?;
    keys(&mut app, "/one<ret>/two").await?;
    let doc = view::doc_mut!(app.editor);
    let edit = editor_core::Transaction::change(doc.text(), [(0, 1, Some("a".into()))].into_iter());
    doc.apply(edit.changes())?;
    keys(&mut app, "<esc>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert!(doc.search_highlight(view.id).is_none());
    Ok(())
}

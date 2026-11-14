use super::helpers::{keys, test_config, AppBuilder};
use editor_core::hashmap;
use std::time::Duration;
use term::{application::TestApplication, keymap, keymap::merge_keys};
use view::{current_ref, document::Mode, editor::Action, quicklist::QuicklistTarget};

fn app() -> anyhow::Result<TestApplication> {
    let mut config = test_config();
    config.editor.idle_timeout = Duration::from_millis(400);
    // No bind by default, use Space L for tests
    let bind =
        || keymap!({"Picker" "space" => { "Space" "L" => search_picker_in_current_buffer } });
    merge_keys(
        &mut config.keys,
        hashmap! { Mode::Normal => bind(), Mode::Select => bind() },
    );
    AppBuilder::new()
        .with_config(config)
        .with_lang_loader(language::syntax::Loader::default())
        .build()
}
async fn capture(app: &mut TestApplication, query: &str) -> anyhow::Result<()> {
    keys(app, "<space>L").await?;
    keys(app, query).await?;
    keys(app, "<C-q>").await
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_unsaved_unicode_navigation_and_quicklist() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("current.txt");
    let other = dir.path().join("other.txt");
    std::fs::write(&path, "disk-only\n")?;
    std::fs::write(&other, "needle\n")?;
    let mut app = app()?;
    app.editor.open(&other, Action::Replace)?;
    let id = app.editor.open(&path, Action::Replace)?;
    keys(&mut app, "%cα needle\nβ needle<esc>").await?;
    capture(&mut app, "needle").await?;
    let entries = app.editor.quicklist.entries();
    assert_eq!(
        entries.len(),
        2,
        "must search unsaved text in this buffer only"
    );
    assert!(entries
        .iter()
        .all(|e| e.target == QuicklistTarget::Path(stdx::path::canonicalize(&path))));
    keys(&mut app, "<ret>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.id(), id);
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .fragment(doc.text().slice(..)),
        "needle"
    );
    assert_eq!(std::fs::read_to_string(path)?, "disk-only\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_scratch_should_search_buffer() -> anyhow::Result<()> {
    let mut app = app()?;
    keys(&mut app, "ineedle<esc>").await?;
    capture(&mut app, "needle").await?;
    assert_eq!(
        app.editor.quicklist.entries().len(),
        1,
        "scratch buffer must be searchable"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_new_file_should_search_buffer() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("not-yet-saved.txt");
    let mut app = app()?;
    app.editor.open(&path, Action::Replace)?;
    keys(&mut app, "ineedle<esc>").await?;
    capture(&mut app, "needle").await?;
    assert_eq!(
        app.editor.quicklist.entries().len(),
        1,
        "new file must be searchable before first save"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_replacement_scope_and_undo() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("current.txt");
    let other = dir.path().join("other.txt");
    std::fs::write(&path, "needle needle\n")?;
    std::fs::write(&other, "needle\n")?;
    let mut app = app()?;
    app.editor.open(&path, Action::Replace)?;
    capture(&mut app, "needle").await?;
    keys(&mut app, "<A-r>changed").await?;
    keys(&mut app, "<C-ret>").await?;
    keys(&mut app, "<esc>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "changed changed\n"
    );
    assert_eq!(std::fs::read_to_string(&path)?, "needle needle\n");
    assert_eq!(std::fs::read_to_string(&other)?, "needle\n");
    keys(&mut app, "u").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "needle needle\n"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_smartcase_and_invalid_regex_recovery() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("current.txt");
    std::fs::write(&path, "needle\nNeedle\n")?;
    let mut app = app()?;
    app.editor.open(&path, Action::Replace)?;
    capture(&mut app, "needle").await?;
    assert_eq!(app.editor.quicklist.entries().len(), 2);
    keys(
        &mut app,
        "<backspace><backspace><backspace><backspace><backspace><backspace>[",
    )
    .await?;
    keys(&mut app, "<backspace>Needle").await?;
    keys(&mut app, "<C-q>").await?;
    assert_eq!(app.editor.quicklist.entries().len(), 1);
    keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_multiline_selection() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("current.txt");
    std::fs::write(&path, "α needle\nβ end\n")?;
    let mut app = app()?;
    app.editor.open(&path, Action::Replace)?;
    capture(&mut app, r"needle\nβ").await?;
    assert_eq!(app.editor.quicklist.entries().len(), 1);
    keys(&mut app, "<ret>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .fragment(doc.text().slice(..)),
        "needle\nβ"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_case_toggle_and_select_mode() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("current.txt");
    std::fs::write(&path, "needle\nNeedle\n")?;
    let mut app = app()?;
    app.editor.open(&path, Action::Replace)?;
    keys(&mut app, "v").await?;
    capture(&mut app, "needle").await?;
    assert_eq!(app.editor.quicklist.entries().len(), 2);
    keys(&mut app, "<A-c>").await?;
    keys(&mut app, "<C-q>").await?;
    assert_eq!(app.editor.quicklist.entries().len(), 1);
    keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_cancel_preserves_selection() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("current.txt");
    std::fs::write(&path, "needle\nother\nneedle\n")?;
    let mut app = app()?;
    app.editor.open(&path, Action::Replace)?;
    keys(&mut app, "jx").await?;
    let (view, doc) = current_ref!(app.editor);
    let before = doc.selection(view.id).clone();
    capture(&mut app, "needle").await?;
    keys(&mut app, "<esc>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.selection(view.id), &before);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_deleted_file_and_visible_contents() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("deleted.txt");
    std::fs::write(&path, "α distinctive needle context\n")?;
    let mut app = app()?;
    app.editor.open(&path, Action::Replace)?;
    std::fs::remove_file(&path)?;
    capture(&mut app, "needle").await?;
    assert_eq!(app.editor.quicklist.entries().len(), 1);
    keys(&mut app, "<C-t>").await?;
    let buffer = app.terminal_backend().buffer();
    let visible = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains("Buffer search"));
    assert!(
        visible.contains("α distinctive needle context"),
        "{visible}"
    );
    keys(&mut app, "<ret>").await?;
    assert!(!path.exists());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_scratch_replacement_refreshes_results() -> anyhow::Result<()> {
    let mut app = app()?;
    keys(&mut app, "ineedle\nneedle<esc>").await?;
    capture(&mut app, "needle").await?;
    assert_eq!(app.editor.quicklist.entries().len(), 2);
    keys(&mut app, "<A-r>changed").await?;
    keys(&mut app, "<ret>").await?;
    keys(&mut app, "<ret>").await?;
    keys(&mut app, "<esc>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "changed\nchanged\n"
    );
    keys(&mut app, "u").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "changed\nneedle\n"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_resume_refreshes_original_document() -> anyhow::Result<()> {
    let mut app = app()?;
    keys(&mut app, "ineedle<esc>").await?;
    let original = current_ref!(app.editor).1.id();
    capture(&mut app, "needle").await?;
    keys(&mut app, "<esc>%cneedle\nneedle<esc>").await?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("other.txt");
    std::fs::write(&path, "unrelated\n")?;
    app.editor.open(&path, Action::Replace)?;
    keys(&mut app, "<space>'").await?;
    keys(&mut app, "<C-q>").await?;
    assert_eq!(app.editor.quicklist.entries().len(), 2);
    assert!(app
        .editor
        .quicklist
        .entries()
        .iter()
        .all(|entry| entry.target == QuicklistTarget::Document(original)));
    keys(&mut app, "<ret>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), original);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_closed_document_cannot_edit_another_buffer() -> anyhow::Result<()> {
    let mut app = app()?;
    keys(&mut app, "ineedle<esc>").await?;
    let original = current_ref!(app.editor).1.id();
    capture(&mut app, "needle").await?;
    keys(&mut app, "<esc>").await?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("other.txt");
    std::fs::write(&path, "needle other\n")?;
    app.editor.open(&path, Action::Replace)?;
    assert!(app.editor.close_document(original, true).is_ok());
    keys(&mut app, "<space>'").await?;
    keys(&mut app, "<C-q>").await?;
    assert!(app.editor.quicklist.entries().is_empty());
    keys(&mut app, "<A-r>changed").await?;
    keys(&mut app, "<C-ret>").await?;
    keys(&mut app, "<esc>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "needle other\n"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_search_picker_crlf_capture_replacement() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("crlf.txt");
    std::fs::write(&path, "α needle\r\nβ needle\r\n")?;
    let mut app = app()?;
    app.editor.open(&path, Action::Replace)?;
    capture(&mut app, "(needle)").await?;
    assert_eq!(app.editor.quicklist.entries().len(), 2);
    keys(&mut app, "<A-r>${1}!<C-ret>").await?;
    keys(&mut app, "<esc>").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "α needle!\r\nβ needle!\r\n"
    );
    keys(&mut app, "u").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "α needle\r\nβ needle\r\n"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

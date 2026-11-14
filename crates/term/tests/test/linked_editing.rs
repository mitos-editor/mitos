//! Linked edits share the input transaction and respect server and session lifetimes.
use anyhow::Context as _;
use editor_core::{Selection, Transaction};
use term::application::TestApplication;
use view::{current, current_ref, document::Mode, editor::Action, features::linked_editing};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};

const TEXT: &str = "😀 <tag>body</tag>\n";

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_yields_to_snippets_before_rendered_coordinates_are_applied(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = application(dir.path(), &[]).await?;
    let (view, doc) = current!(app.editor);
    doc.set_selection(view.id, Selection::point(14));
    assert!(lookup(&mut app).await?.apply(&mut app.editor)?);
    app.editor
        .insert_snippet(&snippets::Snippet::parse("${1:x}$0")?)?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.text(), "😀 <tag>body</txag>\n");
    assert!(doc.active_snippet().is_some());
    assert_eq!(doc.selection(view.id), &Selection::single(14, 15));
    app.editor.insert_char('y')?;
    assert_eq!(current_ref!(app.editor).1.text(), "😀 <tag>body</tyag>\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_command_enters_insert_mode_and_escape_ends_mirroring() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut app = application(dir.path(), &[]).await?;
    super::helpers::keys(&mut app, ":linked-editing<ret>").await?;
    assert_eq!(app.editor.mode(), Mode::Insert);
    super::helpers::keys(&mut app, "x<esc>").await?;
    assert_eq!(current_ref!(app.editor).1.text(), "😀 <txag>body</txag>\n");
    assert_eq!(app.editor.mode(), Mode::Normal);
    super::helpers::keys(&mut app, "iy<esc>").await?;
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .ends_with("</txag>\n"));
    assert!(app.close().await.is_empty());
    Ok(())
}

async fn application(dir: &std::path::Path, args: &[&str]) -> anyhow::Result<TestApplication> {
    let gate = Gate::new(dir.join("initialize-ready"));
    let mut server = ServerConfig::feature("alpha", "--linked-editing", dir).initialize_gate(&gate);
    for arg in args {
        server = server.arg(arg);
    }
    let mut config = test_config();
    config.editor.lsp.enable = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(lsp::syntax_loader(
            "linked-test",
            "linked-test",
            &["alpha"],
            &server.toml(),
        ))
        .build()?;
    let path = dir.join("document.linked-test");
    std::fs::write(&path, TEXT)?;
    app.editor.open(&path, Action::Replace)?;
    let view = current_ref!(app.editor).0.id;
    assert!(linked_editing::prepare(&mut app.editor, view).is_err());
    gate.release()?;
    lsp::initialize(&mut app, 1).await?;
    let (view, doc) = current!(app.editor);
    doc.set_selection(view.id, Selection::point(4));
    Ok(app)
}

async fn lookup(app: &mut TestApplication) -> anyhow::Result<linked_editing::LinkedEditingUpdate> {
    let view = current_ref!(app.editor).0.id;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        linked_editing::prepare(&mut app.editor, view)?.run(),
    )
    .await?
    .context("lookup canceled")
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_mirrors_either_occurrence_in_one_transaction_and_undo_group(
) -> anyhow::Result<()> {
    for cursor in [4, 14] {
        let dir = tempfile::tempdir()?;
        let mut app = application(dir.path(), &[]).await?;
        let (view, doc) = current!(app.editor);
        doc.set_selection(view.id, Selection::point(cursor));
        assert!(lookup(&mut app).await?.apply(&mut app.editor)?);
        assert_eq!(app.editor.mode(), Mode::Insert);
        app.editor.insert_char('x')?;
        let (view, doc) = current_ref!(app.editor);
        assert_eq!(doc.text(), "😀 <txag>body</txag>\n");
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            if cursor == 4 { 5 } else { 16 }
        );
        assert!(doc.active_snippet().is_none());
        lsp::wait_message(
            &lsp::log_path(dir.path(), "alpha"),
            "textDocument/didChange",
        )
        .await?;
        let messages = lsp::read_log(&lsp::log_path(dir.path(), "alpha"))?;
        let changes: Vec<_> = messages
            .iter()
            .filter(|message| message["method"] == "textDocument/didChange")
            .collect();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0]["params"]["contentChanges"][0]["text"],
            "😀 <txag>body</txag>\n"
        );
        let initialize = messages
            .iter()
            .find(|message| message["method"] == "initialize")
            .unwrap();
        assert_eq!(
            initialize["params"]["capabilities"]["textDocument"]["linkedEditingRange"]
                ["dynamicRegistration"],
            false
        );
        let request = messages
            .iter()
            .find(|message| message["method"] == "textDocument/linkedEditingRange")
            .unwrap();
        assert_eq!(request["params"]["position"]["character"], cursor + 1);
        app.editor.delete_char_backward(1)?;
        assert_eq!(current_ref!(app.editor).1.text(), TEXT);
        app.editor.insert_char('z')?;
        let (view, doc) = current!(app.editor);
        assert!(view::editing::undo(doc, view.id));
        assert_eq!(doc.text(), TEXT);
        assert!(!view::editing::undo(doc, view.id));
        assert!(view::editing::redo(doc, view.id));
        assert_eq!(doc.text(), "😀 <tzag>body</tzag>\n");
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_honors_word_patterns_and_preserves_unlinked_input() -> anyhow::Result<()> {
    for (args, character, mirrored) in [
        (vec![], ':', true),
        (vec![], ' ', false),
        (vec!["--no-pattern"], '界', true),
        (vec!["--no-pattern"], '-', false),
    ] {
        let dir = tempfile::tempdir()?;
        let mut app = application(dir.path(), &args).await?;
        assert!(lookup(&mut app).await?.apply(&mut app.editor)?);
        app.editor.insert_char(character)?;
        let expected = format!(
            "😀 <t{character}ag>body</{}>\n",
            if mirrored {
                format!("t{character}ag")
            } else {
                "tag".into()
            }
        );
        assert_eq!(current_ref!(app.editor).1.text().to_string(), expected);
        // Once invalid input ends the session, later valid characters stay local.
        app.editor.insert_char('x')?;
        if !mirrored {
            assert!(current_ref!(app.editor)
                .1
                .text()
                .to_string()
                .ends_with("</tag>\n"));
        }
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_replaces_names_and_retains_one_visible_cursor() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = application(dir.path(), &[]).await?;
    assert!(lookup(&mut app).await?.apply(&mut app.editor)?);
    let (view, doc) = current!(app.editor);
    let replacement = Transaction::change(doc.text(), [(3, 6, Some("my:tag".into()))].into_iter())
        .with_selection(Selection::point(9));
    view::editing::apply(doc, &replacement, view.id)?;
    assert_eq!(doc.text(), "😀 <my:tag>body</my:tag>\n");
    assert_eq!(doc.selection(view.id).len(), 1);
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        9
    );
    let deletion = Transaction::change(doc.text(), [(3, 9, None)].into_iter())
        .with_selection(Selection::point(3));
    view::editing::apply(doc, &deletion, view.id)?;
    assert_eq!(doc.text(), "😀 <>body</>\n");
    app.editor.insert_char('x')?;
    assert_eq!(current_ref!(app.editor).1.text(), "😀 <x>body</x>\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_rejects_malformed_or_missing_server_ranges() -> anyhow::Result<()> {
    for arg in [
        "--reverse",
        "--overlap",
        "--mismatch",
        "--bad-position",
        "--bad-pattern",
        "--null",
        "--unsupported",
    ] {
        let dir = tempfile::tempdir()?;
        let mut app = application(dir.path(), &[arg]).await?;
        if arg == "--unsupported" {
            let view = current_ref!(app.editor).0.id;
            assert!(linked_editing::prepare(&mut app.editor, view).is_err());
        } else {
            assert!(lookup(&mut app).await?.apply(&mut app.editor).is_err());
        }
        assert_eq!(current_ref!(app.editor).1.text(), TEXT);
        assert_eq!(app.editor.mode(), Mode::Normal);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_discards_queued_results_after_context_round_trips() -> anyhow::Result<()> {
    for scenario in [
        "selection",
        "mode",
        "rebound",
        "edited",
        "detached",
        "language",
        "focus",
        "uri",
        "config",
    ] {
        let dir = tempfile::tempdir()?;
        let mut app = application(dir.path(), &[]).await?;
        let update = lookup(&mut app).await?;
        let (view, doc) = current_ref!(app.editor);
        let (view, document, server) = (
            view.id,
            doc.id(),
            doc.language_servers().next().unwrap().id(),
        );
        match scenario {
            "selection" => {
                let doc = app.editor.document_mut(document).unwrap();
                doc.set_selection(view, Selection::point(5));
                doc.set_selection(view, Selection::point(4));
            }
            "mode" => {
                app.editor.set_mode(Mode::Insert);
                app.editor.set_mode(Mode::Normal);
            }
            "rebound" => {
                app.editor.new_file(Action::Replace);
                app.editor.switch(document, Action::Replace);
            }
            "focus" => {
                app.editor.switch(document, Action::VerticalSplit);
                app.editor.focus(view);
            }
            "uri" => app
                .editor
                .set_doc_path(document, &dir.path().join("renamed.linked-test")),
            "config" => {
                let old = (*app.editor.config()).clone();
                app.editor.refresh_config(&old);
            }
            "edited" => {
                let doc = app.editor.document_mut(document).unwrap();
                let change = Transaction::insert(doc.text(), &Selection::point(0), "x".into());
                doc.apply(change.changes())?;
                assert!(view::editing::undo(doc, view));
            }
            "language" => {
                app.editor.set_document_language(document, None)?;
            }
            "detached" => app.editor.handle_language_server_exit(server),
            _ => unreachable!(),
        }
        assert!(!update.apply(&mut app.editor)?);
        assert_eq!(app.editor.document(document).unwrap().text(), TEXT);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_stops_after_escape_cursor_exit_and_document_only_edits(
) -> anyhow::Result<()> {
    for scenario in ["escape", "cursor", "external"] {
        let dir = tempfile::tempdir()?;
        let mut app = application(dir.path(), &[]).await?;
        assert!(lookup(&mut app).await?.apply(&mut app.editor)?);
        match scenario {
            "escape" => {
                app.editor.set_mode(Mode::Normal);
                app.editor.set_mode(Mode::Insert);
            }
            "cursor" => {
                let (view, doc) = current!(app.editor);
                doc.set_selection(view.id, Selection::point(9));
                doc.set_selection(view.id, Selection::point(4));
            }
            "external" => {
                let (_, doc) = current!(app.editor);
                let change =
                    Transaction::change(doc.text(), [(7, 11, Some("BODY".into()))].into_iter());
                doc.apply(change.changes())?;
            }
            _ => unreachable!(),
        }
        app.editor.insert_char('x')?;
        assert!(current_ref!(app.editor)
            .1
            .text()
            .to_string()
            .ends_with("</tag>\n"));
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

//! Linked edits share the input transaction and respect server and session lifetimes.
use editor_core::{Selection, Transaction};
use term::application::TestApplication;
use view::{current, current_ref, document::Mode, editor::Action, features::linked_editing};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config,
};

const TEXT: &str = "😀 <tag>body</tag>\n";

#[tokio::test(flavor = "multi_thread")]
async fn automatic_linked_edits_preserve_normal_mode_and_replacement_selections(
) -> anyhow::Result<()> {
    for (cursor, start) in [(4, 3), (14, 13)] {
        let dir = tempfile::tempdir()?;
        let mut f = AutomaticFixture::new(dir.path(), &[], cursor, true).await?;
        f.response().await?(&mut f.app.editor);
        let (view, doc) = current!(f.app.editor);
        doc.set_selection(view.id, Selection::single(start, start + 3));
        let change = Transaction::change(
            doc.text(),
            [(start, start + 3, Some("button".into()))].into_iter(),
        )
        .with_selection(Selection::single(start + 6, start));
        view::editing::apply(doc, &change, view.id)?;
        assert_eq!(doc.text(), "😀 <button>body</button>\n");
        assert_eq!(
            doc.selection(view.id),
            &Selection::single(
                if cursor == 4 { 9 } else { 22 },
                if cursor == 4 { 3 } else { 16 }
            )
        );
        assert_eq!(f.app.editor.mode(), Mode::Normal);
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_linked_editing_can_be_disabled_and_reenabled_at_runtime() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = AutomaticFixture::new(dir.path(), &[], 4, true).await?;
    f.response().await?(&mut f.app.editor);
    super::helpers::keys(&mut f.app, ":set lsp.linked-editing false<ret>ix<esc>").await?;
    assert_eq!(current_ref!(f.app.editor).1.text(), "😀 <txag>body</tag>\n");
    let (view, doc) = current!(f.app.editor);
    assert!(view::editing::undo(doc, view.id));
    super::helpers::keys(&mut f.app, ":set lsp.linked-editing true<ret>").await?;
    f.response().await?(&mut f.app.editor);
    super::helpers::keys(&mut f.app, "ix<esc>").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.text(),
        "😀 <txag>body</txag>\n"
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_linked_editing_retries_discovery_dropped_while_unfocused() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = AutomaticFixture::new(dir.path(), &[], 4, true).await?;
    let old = f.response().await?;
    let (view, doc) = current_ref!(f.app.editor);
    let (view, document) = (view.id, doc.id());
    f.app.editor.new_file(Action::VerticalSplit);
    old(&mut f.app.editor);
    f.app
        .editor
        .document_mut(document)
        .unwrap()
        .set_selection(view, Selection::point(5));
    f.response().await?(&mut f.app.editor);
    f.app.editor.focus(view);
    f.response().await?(&mut f.app.editor);
    super::helpers::keys(&mut f.app, "ix<esc>").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.text(),
        "😀 <taxg>body</taxg>\n"
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_linked_editing_keeps_plain_completions_and_imports_in_sync() -> anyhow::Result<()>
{
    for (cursor, imports, typing, snippet) in [
        (4, false, false, false),
        (14, false, false, false),
        (4, true, false, false),
        (14, true, false, false),
        (4, false, true, false),
        (4, false, false, true),
        (14, false, false, true),
    ] {
        let dir = tempfile::tempdir()?;
        let args: &[&str] = if snippet {
            &["--linked-completion", "--linked-completion-final"]
        } else if imports {
            &["--linked-completion", "--linked-completion-import"]
        } else {
            &["--linked-completion"]
        };
        let mut f = AutomaticFixture::new(dir.path(), args, cursor, true).await?;
        f.response().await?(&mut f.app.editor);
        super::helpers::keys(&mut f.app, "i<C-x>").await?;
        assert!(view::features::completion::is_shown(&f.app.editor));
        if typing {
            super::helpers::keys(&mut f.app, "a").await?;
        }
        super::helpers::keys(&mut f.app, "<tab><ret>").await?;
        let prefix = if imports { "imported\n" } else { "" };
        assert_eq!(
            current_ref!(f.app.editor).1.text().to_string(),
            format!("{prefix}😀 <tagged>body</tagged>\n")
        );
        f.app.editor.insert_char('z')?;
        assert_eq!(
            current_ref!(f.app.editor).1.text().to_string(),
            if snippet {
                format!("{prefix}😀 <taggedz>body</taggedz>\n")
            } else {
                format!("{prefix}😀 <taggezd>body</taggezd>\n")
            }
        );
        let (view, doc) = current!(f.app.editor);
        assert!(view::editing::undo(doc, view.id));
        assert_eq!(doc.text(), TEXT);
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

struct AutomaticFixture {
    app: TestApplication,
    callbacks: tokio::sync::mpsc::UnboundedReceiver<(bool, view::callbacks::EditorCallback)>,
    log: std::path::PathBuf,
}

impl AutomaticFixture {
    async fn new(
        dir: &std::path::Path,
        args: &[&str],
        cursor: usize,
        enabled: bool,
    ) -> anyhow::Result<Self> {
        let mut config = test_config();
        config.editor.lsp.linked_editing = enabled;
        let (mut app, callbacks, gate) = lsp::feature_app(
            dir,
            TEXT,
            "linked-test",
            &["alpha"],
            config,
            |name| {
                let mut server = ServerConfig::feature(name, "--linked-editing", dir);
                for arg in args {
                    server = server.arg(arg);
                }
                if args.contains(&"--hold-linked") {
                    server = server
                        .arg("--linked-response-gate")
                        .arg(dir.join("linked-response-ready"));
                }
                server
            },
            |handlers, sender, _| {
                handlers.linked_editing = linked_editing::LinkedEditingHandler::new(sender)
            },
        )?;
        let (view, doc) = current!(app.editor);
        doc.set_selection(view.id, Selection::point(cursor));
        gate.release()?;
        lsp::initialize(&mut app, 1).await?;
        Ok(Self {
            app,
            callbacks,
            log: lsp::log_path(dir, "alpha"),
        })
    }

    async fn response(&mut self) -> anyhow::Result<view::callbacks::EditorCallback> {
        super::helpers::callbacks::response(&mut self.app, &mut self.callbacks).await
    }

    fn requests(&self) -> anyhow::Result<usize> {
        Ok(lsp::read_log(&self.log)?
            .iter()
            .filter(|message| message["method"] == "textDocument/linkedEditingRange")
            .count())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_automatically_mirrors_typing_without_a_command() -> anyhow::Result<()> {
    assert!(editor_config::Config::default().lsp.linked_editing);
    for cursor in [4, 14] {
        let dir = tempfile::tempdir()?;
        let mut f = AutomaticFixture::new(dir.path(), &[], cursor, true).await?;
        let original = current_ref!(f.app.editor)
            .1
            .selection(current_ref!(f.app.editor).0.id)
            .clone();
        f.response().await?(&mut f.app.editor);
        assert_eq!(f.app.editor.mode(), Mode::Normal);
        assert_eq!(
            current_ref!(f.app.editor)
                .1
                .selection(current_ref!(f.app.editor).0.id),
            &original
        );
        super::helpers::keys(&mut f.app, "ix<esc>").await?;
        assert_eq!(
            current_ref!(f.app.editor).1.text(),
            "😀 <txag>body</txag>\n"
        );
        assert_eq!(f.requests()?, 1);
        let (view, doc) = current!(f.app.editor);
        assert_eq!(doc.selection(view.id).len(), 1);
        assert!(view::editing::undo(doc, view.id));
        assert_eq!(doc.text(), TEXT);
        assert!(view::editing::redo(doc, view.id));
        assert_eq!(doc.text(), "😀 <txag>body</txag>\n");
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_catches_up_typing_before_the_server_replies() -> anyhow::Result<()> {
    for change_word in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut f = AutomaticFixture::new(dir.path(), &["--hold-linked"], 4, true).await?;
        lsp::wait_message(&f.log, "textDocument/linkedEditingRange").await?;
        if change_word {
            let (view, doc) = current!(f.app.editor);
            doc.set_selection(view.id, Selection::single(3, 6));
            f.app.editor.change_selections(None, 1)?;
        } else {
            f.app.editor.enter_insert_mode();
        }
        for c in ['x', 'y'] {
            f.app.editor.insert_char(c)?;
        }
        let expected = if change_word {
            "😀 <xy>body</xy>\n"
        } else {
            "😀 <txyag>body</txyag>\n"
        };
        assert!(current_ref!(f.app.editor)
            .1
            .text()
            .to_string()
            .ends_with("</tag>\n"));
        Gate::new(dir.path().join("linked-response-ready")).release()?;
        f.response().await?(&mut f.app.editor);
        assert_eq!(current_ref!(f.app.editor).1.text(), expected);
        assert_eq!(f.app.editor.mode(), Mode::Insert);
        let (view, doc) = current!(f.app.editor);
        assert!(view::editing::undo(doc, view.id));
        assert_eq!(doc.text(), TEXT);
        assert!(view::editing::redo(doc, view.id));
        assert_eq!(doc.text(), expected);
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_linked_editing_is_quiet_when_disabled_or_unavailable() -> anyhow::Result<()> {
    for (args, enabled) in [
        (vec![], false),
        (vec!["--unsupported"], true),
        (vec!["--null"], true),
        (vec!["--bad-pattern"], true),
        (vec!["--mismatch"], true),
        (vec!["--reverse"], true),
        (vec!["--overlap"], true),
        (vec!["--bad-position"], true),
    ] {
        let dir = tempfile::tempdir()?;
        let mut f = AutomaticFixture::new(dir.path(), &args, 4, enabled).await?;
        if enabled && !args.contains(&"--unsupported") {
            f.response().await?(&mut f.app.editor);
        }
        assert_eq!(current_ref!(f.app.editor).1.text(), TEXT);
        assert_eq!(f.app.editor.mode(), Mode::Normal);
        super::helpers::assert_status_not_error(&f.app.editor);
        if !enabled || args.contains(&"--unsupported") {
            assert_eq!(f.requests()?, 0);
        }
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_linked_editing_restarts_a_lookup_canceled_without_typing() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = AutomaticFixture::new(dir.path(), &["--hold-linked"], 4, true).await?;
    lsp::wait_message(&f.log, "textDocument/linkedEditingRange").await?;
    f.app.editor.enter_insert_mode();
    f.app.editor.enter_normal_mode();
    f.app.editor.enter_insert_mode();
    f.app.editor.insert_char('x')?;
    Gate::new(dir.path().join("linked-response-ready")).release()?;
    f.response().await?(&mut f.app.editor);
    assert_eq!(
        current_ref!(f.app.editor).1.text(),
        "😀 <txag>body</txag>\n"
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_linked_editing_reuses_cached_ranges_at_the_other_name() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = AutomaticFixture::new(dir.path(), &[], 4, true).await?;
    f.response().await?(&mut f.app.editor);
    let (view, doc) = current!(f.app.editor);
    doc.set_selection(view.id, Selection::point(14));
    super::helpers::keys(&mut f.app, "ix<esc>").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.text(),
        "😀 <txag>body</txag>\n"
    );
    assert_eq!(f.requests()?, 1);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_linked_editing_keeps_a_cold_lookup_while_moving_within_the_name(
) -> anyhow::Result<()> {
    for cursor in [4, 14] {
        let dir = tempfile::tempdir()?;
        let mut f = AutomaticFixture::new(dir.path(), &["--hold-linked"], cursor, true).await?;
        lsp::wait_message(&f.log, "textDocument/linkedEditingRange").await?;
        f.app.editor.enter_insert_mode();
        f.app.editor.insert_char('x')?;
        let (view, doc) = current!(f.app.editor);
        doc.set_selection(view.id, Selection::point(cursor));
        f.app.editor.insert_char('y')?;
        Gate::new(dir.path().join("linked-response-ready")).release()?;
        f.response().await?(&mut f.app.editor);
        assert_eq!(
            current_ref!(f.app.editor).1.text(),
            "😀 <tyxag>body</tyxag>\n"
        );
        assert_eq!(f.requests()?, 1);
        let (view, doc) = current_ref!(f.app.editor);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            if cursor == 4 { 5 } else { 17 }
        );
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_linked_editing_rejects_late_results_after_leaving_the_edit() -> anyhow::Result<()>
{
    for scenario in [
        "escape",
        "cursor",
        "focus",
        "external",
        "committed",
        "language",
        "uri",
        "config",
        "detached",
        "rebound",
        "unrelated",
    ] {
        let dir = tempfile::tempdir()?;
        let mut f = AutomaticFixture::new(dir.path(), &["--hold-linked"], 4, true).await?;
        lsp::wait_message(&f.log, "textDocument/linkedEditingRange").await?;
        f.app.editor.enter_insert_mode();
        f.app.editor.insert_char('x')?;
        Gate::new(dir.path().join("linked-response-ready")).release()?;
        let response = f.response().await?;
        let view = current_ref!(f.app.editor).0.id;
        let document = current_ref!(f.app.editor).1.id();
        match scenario {
            "escape" => f.app.editor.enter_normal_mode(),
            "cursor" => current!(f.app.editor)
                .1
                .set_selection(view, Selection::point(9)),
            "focus" => {
                f.app.editor.new_file(Action::VerticalSplit);
                f.app.editor.focus(view);
            }
            "external" => {
                let doc = current!(f.app.editor).1;
                let transaction =
                    Transaction::insert(doc.text(), &Selection::point(0), "outside ".into());
                doc.apply(transaction.changes())?;
            }
            "committed" => {
                current!(f.app.editor).1.commit_changes();
            }
            "language" => f.app.editor.set_document_language(document, None)?,
            "uri" => f
                .app
                .editor
                .set_doc_path(document, &dir.path().join("renamed.linked-test")),
            "config" => {
                let old = (*f.app.editor.config()).clone();
                f.app.editor.refresh_config(&old);
            }
            "detached" => {
                let server = current_ref!(f.app.editor)
                    .1
                    .language_servers()
                    .next()
                    .unwrap()
                    .id();
                f.app.editor.handle_language_server_exit(server);
            }
            "rebound" => {
                f.app.editor.new_file(Action::Replace);
                f.app.editor.switch(document, Action::Replace);
            }
            "unrelated" => {
                let doc = current!(f.app.editor).1;
                let edit =
                    Transaction::insert(doc.text(), &Selection::point(10), "body-edit".into());
                view::editing::apply(doc, &edit, view)?;
            }
            _ => unreachable!(),
        }
        response(&mut f.app.editor);
        assert!(current_ref!(f.app.editor)
            .1
            .text()
            .to_string()
            .ends_with("</tag>\n"));
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_yields_to_snippets_before_rendered_coordinates_are_applied(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = AutomaticFixture::new(dir.path(), &[], 14, true).await?;
    f.response().await?(&mut f.app.editor);
    f.app.editor.enter_insert_mode();
    f.app
        .editor
        .insert_snippet(&snippets::Snippet::parse("${1:x}$0")?)?;
    let (view, doc) = current_ref!(f.app.editor);
    assert_eq!(doc.text(), "😀 <tag>body</txag>\n");
    assert!(doc.active_snippet().is_some());
    assert_eq!(doc.selection(view.id), &Selection::single(14, 15));
    f.app.editor.insert_char('y')?;
    assert_eq!(current_ref!(f.app.editor).1.text(), "😀 <tag>body</tyag>\n");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_mirrors_either_occurrence_in_one_transaction_and_undo_group(
) -> anyhow::Result<()> {
    for cursor in [4, 14] {
        let dir = tempfile::tempdir()?;
        let mut f = AutomaticFixture::new(dir.path(), &[], cursor, true).await?;
        f.response().await?(&mut f.app.editor);
        f.app.editor.enter_insert_mode();
        assert_eq!(f.app.editor.mode(), Mode::Insert);
        f.app.editor.insert_char('x')?;
        let (view, doc) = current_ref!(f.app.editor);
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
        f.app.editor.delete_char_backward(1)?;
        assert_eq!(current_ref!(f.app.editor).1.text(), TEXT);
        f.app.editor.insert_char('z')?;
        let (view, doc) = current!(f.app.editor);
        assert!(view::editing::undo(doc, view.id));
        assert_eq!(doc.text(), TEXT);
        assert!(!view::editing::undo(doc, view.id));
        assert!(view::editing::redo(doc, view.id));
        assert_eq!(doc.text(), "😀 <tzag>body</tzag>\n");
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_honors_word_patterns_and_preserves_unlinked_input() -> anyhow::Result<()> {
    for (args, character, mirrored) in [
        (vec![], ':', true),
        (vec![], ' ', false),
        (vec!["--no-pattern"], '界', true),
        (vec!["--no-pattern"], '-', true),
        (vec!["--no-pattern"], '/', false),
    ] {
        let dir = tempfile::tempdir()?;
        let mut f = AutomaticFixture::new(dir.path(), &args, 4, true).await?;
        f.response().await?(&mut f.app.editor);
        f.app.editor.enter_insert_mode();
        f.app.editor.insert_char(character)?;
        let expected = format!(
            "😀 <t{character}ag>body</{}>\n",
            if mirrored {
                format!("t{character}ag")
            } else {
                "tag".into()
            }
        );
        assert_eq!(current_ref!(f.app.editor).1.text().to_string(), expected);
        // Once invalid input ends the session, later valid characters stay local.
        f.app.editor.insert_char('x')?;
        if !mirrored {
            assert!(current_ref!(f.app.editor)
                .1
                .text()
                .to_string()
                .ends_with("</tag>\n"));
        }
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_editing_replaces_names_and_retains_one_visible_cursor() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = AutomaticFixture::new(dir.path(), &[], 4, true).await?;
    f.response().await?(&mut f.app.editor);
    f.app.editor.enter_insert_mode();
    let (view, doc) = current!(f.app.editor);
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
    f.app.editor.insert_char('x')?;
    assert_eq!(current_ref!(f.app.editor).1.text(), "😀 <x>body</x>\n");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

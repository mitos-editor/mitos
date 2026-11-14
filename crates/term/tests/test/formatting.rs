//! Exercise shared document and range formatting against the real LSP transport.
use editor_core::Selection;
use term::application::TestApplication;
use view::{current, current_ref, editor::Action, features::formatting};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};

async fn application(dir: &std::path::Path) -> anyhow::Result<TestApplication> {
    application_with_args(dir, &[]).await
}

async fn application_with_args(
    dir: &std::path::Path,
    args: &[&str],
) -> anyhow::Result<TestApplication> {
    let gate = Gate::new(dir.join("initialize-ready"));
    let mut server = ServerConfig::feature("alpha", "--formatting", dir).initialize_gate(&gate);
    for arg in args {
        server = server.arg(arg);
    }
    let mut servers = server.toml();
    if args.contains(&"--ignore-formatting") {
        servers.push_str("timeout = 1\n");
    }
    let mut config = test_config();
    config.editor.lsp.enable = true;
    config.editor.format.on_save = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(lsp::syntax_loader(
            "formatting-test",
            "formatting-test",
            &["alpha"],
            &servers,
        ))
        .build()?;
    let path = dir.join("document.formatting-test");
    std::fs::write(&path, "😀 foo bar\n")?;
    app.editor.open(&path, Action::Replace)?;
    gate.release()?;
    let (server, call) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        app.editor.next_language_server_message(),
    )
    .await?
    .expect("server initialized");
    assert!(matches!(&call, lsp_client::Call::Notification(n) if n.method == "initialized"));
    assert!(app
        .editor
        .language_server_by_id(server)
        .unwrap()
        .is_initialized());
    // Wire capabilities alone must not permit requests before configuration/didOpen.
    assert!(formatting::prepare_selections(&app.editor, current_ref!(app.editor).0.id).is_err());
    app.handle_language_server_message(call, server).await;
    let (view, doc) = current!(app.editor);
    doc.set_selection(view.id, Selection::single(2, 5));
    Ok(app)
}

#[tokio::test(flavor = "multi_thread")]
async fn document_formatting_reports_server_errors_only_for_current_targets() -> anyhow::Result<()>
{
    for stale in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut app = application_with_args(
            dir.path(),
            &["--document-formatting", "--reject-formatting"],
        )
        .await?;
        let view = current_ref!(app.editor).0.id;
        let update = formatting::prepare_document(&app.editor, view)?.run().await;
        if stale {
            app.editor.new_file(Action::Replace);
        }
        let result = update.apply(&mut app.editor);
        if stale {
            assert!(!result?);
        } else {
            let error = result.expect_err("server formatting failures must reach presentation");
            assert!(error.to_string().contains("alpha"));
            assert!(error.to_string().contains("test formatting failure"));
            assert_eq!(current_ref!(app.editor).1.text(), "😀 foo bar\n");
        }
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn document_formatting_reports_language_server_timeouts() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = application_with_args(
        dir.path(),
        &["--document-formatting", "--ignore-formatting"],
    )
    .await?;
    let view = current_ref!(app.editor).0.id;
    let update = formatting::prepare_document(&app.editor, view)?.run().await;
    let error = update
        .apply(&mut app.editor)
        .expect_err("formatting timeouts must reach presentation");
    assert!(error.to_string().contains("alpha"));
    assert!(error.to_string().contains("timed out"));
    assert_eq!(current_ref!(app.editor).1.text(), "😀 foo bar\n");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_lsp_formatting_still_saves_the_document() -> anyhow::Result<()> {
    use view::save::{self, WriteOptions};
    let dir = tempfile::tempdir()?;
    let mut app = application_with_args(
        dir.path(),
        &["--document-formatting", "--reject-formatting"],
    )
    .await?;
    let (view, doc) = current!(app.editor);
    let (view, id, path) = (view.id, doc.id(), doc.path().unwrap().to_owned());
    let edit = editor_core::Transaction::insert(doc.text(), &Selection::point(0), "new ".into());
    doc.apply(edit.changes())?;
    let request = save::prepare(
        &mut app.editor,
        id,
        Some(view),
        None,
        WriteOptions {
            force: false,
            auto_format: true,
            code_actions: false,
        },
    )?;
    app.editor.submit_save(request)?;
    app.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(path)?, "new 😀 foo bar\n");
    assert!(!app.editor.document(id).unwrap().is_modified());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn range_formatting_converts_utf16_positions_and_preserves_surrounding_text(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = application(dir.path()).await?;
    let (view, doc) = current!(app.editor);
    doc.indent_style = editor_core::indent::IndentStyle::Spaces(2);
    assert_ne!(doc.tab_width(), doc.indent_width());
    let view = view.id;
    let update = formatting::prepare_selections(&app.editor, view)?
        .run()
        .await;
    assert!(update.apply(&mut app.editor)?);
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "😀 formatted bar\n"
    );
    let log = std::fs::read_to_string(lsp::log_path(dir.path(), "alpha"))?;
    let request = log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|message| message["method"] == "textDocument/rangeFormatting")
        .unwrap();
    assert_eq!(request["params"]["options"]["tabSize"], 2);
    assert_eq!(request["params"]["range"]["start"]["character"], 3);
    assert_eq!(request["params"]["range"]["end"]["character"], 6);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn range_formatting_rejects_rebound_views_and_detached_servers() -> anyhow::Result<()> {
    for rebind in [true, false] {
        let dir = tempfile::tempdir()?;
        let mut app = application(dir.path()).await?;
        let (view, doc) = current_ref!(app.editor);
        let (view, id) = (view.id, doc.id());
        let server = doc.language_servers().next().unwrap().id();
        let update = formatting::prepare_selections(&app.editor, view)?
            .run()
            .await;
        if rebind {
            app.editor.new_file(Action::Replace);
            app.editor.switch(id, Action::Replace);
        } else {
            app.editor.handle_language_server_exit(server);
        }
        assert!(!update.apply(&mut app.editor)?);
        assert_eq!(
            app.editor.document(id).unwrap().text().to_string(),
            "😀 foo bar\n"
        );
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn document_formatting_rejects_detached_and_replaced_servers() -> anyhow::Result<()> {
    for lifecycle in ["current", "detach", "restart"] {
        let dir = tempfile::tempdir()?;
        let mut app = application_with_args(dir.path(), &["--document-formatting"]).await?;
        let (view, doc) = current_ref!(app.editor);
        let (view, document, server) = (
            view.id,
            doc.id(),
            doc.language_servers().next().unwrap().id(),
        );
        let update = formatting::prepare_document(&app.editor, view)?.run().await;
        if lifecycle == "detach" {
            app.editor.handle_language_server_exit(server);
        } else if lifecycle == "restart" {
            app.editor.restart_language_servers(document, &[])?;
            let replacement = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let (server, call) = app
                        .editor
                        .next_language_server_message()
                        .await
                        .expect("replacement server initialized");
                    let initialized = matches!(&call, lsp_client::Call::Notification(n) if n.method == "initialized");
                    app.handle_language_server_message(call, server).await;
                    if initialized {
                        break server;
                    }
                }
            })
            .await?;
            assert_ne!(server, replacement);
        }
        assert_eq!(update.apply(&mut app.editor)?, lifecycle == "current");
        assert_eq!(
            app.editor.document(document).unwrap().text().to_string(),
            if lifecycle == "current" {
                "😀 formatted bar\n"
            } else {
                "😀 foo bar\n"
            }
        );
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn detached_save_formatter_is_discarded_without_canceling_the_write() -> anyhow::Result<()> {
    use view::save::{self, SaveProgress, WriteOptions};
    for detached in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut app = application_with_args(dir.path(), &["--document-formatting"]).await?;
        let (view, doc) = current_ref!(app.editor);
        let (view, document, server, path) = (
            view.id,
            doc.id(),
            doc.language_servers().next().unwrap().id(),
            doc.path().unwrap().to_owned(),
        );
        let request = save::prepare(
            &mut app.editor,
            document,
            Some(view),
            None,
            WriteOptions {
                force: false,
                auto_format: true,
                code_actions: false,
            },
        )?;
        let SaveProgress::Pending(task) = save::submit(&mut app.editor, request)? else {
            anyhow::bail!("expected formatting stage")
        };
        let update = task.run().await?;
        if detached {
            app.editor.handle_language_server_exit(server);
        }
        assert!(matches!(
            update.apply(&mut app.editor)?,
            SaveProgress::Queued
        ));
        app.editor.flush_writes().await?;
        let expected = if detached {
            "😀 foo bar\n"
        } else {
            "😀 formatted bar\n"
        };
        assert_eq!(std::fs::read_to_string(path)?, expected);
        assert_eq!(app.editor.document(document).unwrap().text(), expected);
        assert!(!app.editor.document(document).unwrap().is_modified());
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

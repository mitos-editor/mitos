use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use ::diagnostics::DiagnosticProvider;
use editor_core::{Transaction, Uri};
use event::register_hook;
use lsp_client::{lsp, Call, Notification};
use serde_json::{json, Value};
use view::{
    current, current_ref,
    events::{
        DocumentDidChange, DocumentDidClose, LanguageServerExited, LanguageServerInitialized,
    },
};

use super::helpers::lsp::{read_log, Fixture};

enum ObserverMarker {}
impl lsp::notification::Notification for ObserverMarker {
    type Params = Value;
    const METHOD: &'static str = "test/observer";
}

fn params(uri: &Uri, version: Option<i32>, message: &str) -> lsp::PublishDiagnosticsParams {
    serde_json::from_value(json!({
        "uri": uri.to_url().unwrap().to_string(), "version": version,
        "diagnostics": [{"range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 6}}, "severity": 2, "message": message}]
    })).unwrap()
}

fn notification(method: &str, params: Value) -> Call {
    serde_json::from_value(json!({"jsonrpc": "2.0", "method": method, "params": params})).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn initialization_sends_configuration_before_open_and_feature_requests() -> anyhow::Result<()>
{
    event::register_event::<LanguageServerInitialized>();
    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        event
            .editor
            .language_server_by_id(event.server_id)
            .unwrap()
            .notify::<ObserverMarker>(json!({"phase": "initialized"}));
        Ok(())
    });
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha"])?;
    f.initialize().await?;
    let (id, call) = f.next().await?;
    let Call::Notification(message) = call else {
        anyhow::bail!("expected diagnostic notification")
    };
    let Notification::PublishDiagnostics(params) =
        Notification::parse(&message.method, message.params)?
    else {
        anyhow::bail!("expected push diagnostics")
    };
    f.app.editor.handle_publish_diagnostics(id, params);
    assert_eq!(f.messages(), ["alpha"]);
    let diagnostic = &current_ref!(f.app.editor).1.diagnostics()[0];
    assert_eq!((diagnostic.range.start, diagnostic.range.end), (2, 5));
    let messages = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let messages: Vec<Value> = read_log(&f.logs[0])?;
            if messages
                .iter()
                .any(|m| m["method"] == "textDocument/documentSymbol")
            {
                return anyhow::Ok(messages);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await??;
    let index = |method| {
        messages
            .iter()
            .position(|message| message["method"] == method)
            .unwrap()
    };
    let config = index("workspace/didChangeConfiguration");
    assert!(index("initialized") < config);
    assert!(config < index("textDocument/didOpen"));
    assert!(index("textDocument/didOpen") < index("textDocument/documentSymbol"));
    assert!(index("textDocument/didOpen") < index("test/observer"));
    assert_eq!(
        messages[config]["params"]["settings"],
        json!({"lifecycle": "alpha"})
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pushes_reject_uninitialized_unknown_invalid_and_stale_updates() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha"])?;
    let id = f.server("alpha");
    let uri = f.uri();
    assert!(!f
        .app
        .editor
        .language_server_by_id(id)
        .unwrap()
        .is_initialized());
    f.app
        .editor
        .handle_publish_diagnostics(id, params(&uri, Some(0), "too early"));
    assert!(f.app.editor.diagnostics().is_empty());
    f.initialize().await?;
    f.app
        .editor
        .handle_publish_diagnostics(id, params(&uri, Some(0), "current"));
    assert_eq!(f.messages(), ["current"]);
    let (view, doc) = current!(f.app.editor);
    let transaction = Transaction::insert(doc.text(), doc.selection(view.id), "x".into());
    assert!(view::editing::apply(doc, &transaction, view.id));
    let version = doc.version();
    for stale in [version - 1, version + 1] {
        f.app
            .editor
            .handle_publish_diagnostics(id, params(&uri, Some(stale), "stale"));
        assert_eq!(f.messages(), ["current"]);
        assert_eq!(f.app.editor.diagnostics()[&uri][0].0.message, "current");
    }
    let mut invalid = params(&uri, None, "invalid URI");
    invalid.uri = "https://example.test/diagnostic".parse()?;
    f.app.editor.handle_publish_diagnostics(id, invalid);
    assert_eq!(f.app.editor.diagnostics().len(), 1);
    f.app
        .editor
        .handle_publish_diagnostics(id, params(&uri, None, "unversioned"));
    assert_eq!(f.messages(), ["unversioned"]);
    let mut clear = params(&uri, Some(version), "unused");
    clear.diagnostics.clear();
    f.app.editor.handle_publish_diagnostics(id, clear);
    assert!(f.messages().is_empty());
    f.app.editor.handle_language_server_exit(id);
    f.app
        .editor
        .handle_publish_diagnostics(id, params(&uri, None, "after exit"));
    f.app.editor.handle_language_server_initialized(id);
    assert!(f.app.editor.diagnostics().is_empty());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_cleans_open_and_unopened_diagnostics_before_hooks_and_registry_removal(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha", "beta"])?;
    f.initialize().await?;
    let alpha = f.server("alpha");
    let beta = f.server("beta");
    let open = f.uri();
    // Match the normalization applied when diagnostic URLs become URI map keys.
    // Windows temporary directories can contain a verbatim path prefix.
    let shared = Uri::from(stdx::path::normalize(
        dir.path().join("unopened-shared.lifecycle-test"),
    ));
    let alpha_only = Uri::from(stdx::path::normalize(
        dir.path().join("unopened-alpha.lifecycle-test"),
    ));
    for uri in [&open, &shared] {
        f.app
            .editor
            .handle_publish_diagnostics(alpha, params(uri, None, "alpha"));
        f.app
            .editor
            .handle_publish_diagnostics(beta, params(uri, None, "beta"));
    }
    f.app
        .editor
        .handle_publish_diagnostics(alpha, params(&alpha_only, None, "alpha"));
    let pull = DiagnosticProvider::Lsp {
        server_id: alpha,
        identifier: Some("pull".into()),
    };
    f.app.editor.handle_lsp_diagnostics(
        &pull,
        open.clone(),
        None,
        params(&open, None, "alpha pull").diagnostics,
    );
    let doc = current!(f.app.editor).1;
    let mut spelling = doc.diagnostics()[0].clone();
    spelling.provider = DiagnosticProvider::Spelling;
    spelling.message = "spelling".into();
    doc.replace_diagnostics([spelling], &[], Some(&DiagnosticProvider::Spelling));
    let exits = Arc::new(AtomicUsize::new(0));
    let seen = exits.clone();
    event::register_hook!(move |event: &mut LanguageServerExited<'_>| {
        assert_eq!(event.server_id, alpha);
        assert!(event.editor.language_server_by_id(alpha).is_some());
        assert!(event
            .editor
            .documents()
            .any(|doc| doc.supports_language_server(alpha)));
        assert!(event
            .editor
            .diagnostics()
            .values()
            .flatten()
            .all(|(_, provider)| provider.language_server_id() != Some(alpha)));
        assert!(event
            .editor
            .documents()
            .flat_map(|doc| doc.diagnostics())
            .all(|diagnostic| diagnostic.provider.language_server_id() != Some(alpha)));
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    f.app.editor.handle_language_server_exit(alpha);
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert!(f.app.editor.language_server_by_id(alpha).is_none());
    assert!(f.app.editor.language_server_by_id(beta).is_some());
    assert_eq!(f.messages(), ["beta", "spelling"]);
    assert!(!f.app.editor.diagnostics().contains_key(&alpha_only));
    assert_eq!(f.app.editor.diagnostics()[&shared].len(), 1);
    assert_eq!(f.app.editor.diagnostics()[&shared][0].0.message, "beta");
    f.app.editor.handle_language_server_exit(alpha);
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_clears_selected_server_document_and_workspace_diagnostics() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha", "beta"])?;
    f.initialize().await?;
    let alpha = f.server("alpha");
    let beta = f.server("beta");
    let open = f.uri();
    let other_path = dir.path().join("other.lifecycle-test");
    std::fs::write(&other_path, "😀 original\n")?;
    let other_document = f.app.editor.open(&other_path, view::editor::Action::Load)?;
    let other = f
        .app
        .editor
        .document(other_document)
        .unwrap()
        .uri()
        .unwrap();
    let shared = Uri::from(stdx::path::normalize(
        dir.path().join("unopened-shared.lifecycle-test"),
    ));
    let alpha_only = Uri::from(stdx::path::normalize(
        dir.path().join("unopened-alpha.lifecycle-test"),
    ));
    for uri in [&open, &other, &shared] {
        for (id, name) in [(alpha, "alpha"), (beta, "beta")] {
            f.app
                .editor
                .handle_publish_diagnostics(id, params(uri, None, name));
        }
    }
    f.app
        .editor
        .handle_publish_diagnostics(alpha, params(&alpha_only, None, "alpha"));
    let doc = current!(f.app.editor).1;
    let document = doc.id();
    let mut spelling = doc.diagnostics()[0].clone();
    spelling.provider = DiagnosticProvider::Spelling;
    spelling.message = "spelling".into();
    doc.replace_diagnostics([spelling], &[], Some(&DiagnosticProvider::Spelling));

    let exits = Arc::new(AtomicUsize::new(0));
    let seen = exits.clone();
    event::register_hook!(move |event: &mut LanguageServerExited<'_>| {
        assert_eq!(event.server_id, alpha);
        assert!(event.editor.language_server_by_id(alpha).is_some());
        assert!(event
            .editor
            .document(document)
            .unwrap()
            .supports_language_server(alpha));
        assert!(event
            .editor
            .diagnostics()
            .values()
            .flatten()
            .all(|(_, provider)| provider.language_server_id() != Some(alpha)));
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    f.app
        .editor
        .restart_language_servers(document, &["alpha"])?;

    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert_ne!(f.server("alpha"), alpha);
    assert_eq!(f.server("beta"), beta);
    assert_eq!(f.messages(), ["beta", "spelling"]);
    let other_doc = f.app.editor.document(other_document).unwrap();
    assert_eq!(other_doc.diagnostics().len(), 1);
    assert_eq!(other_doc.diagnostics()[0].message.as_ref(), "beta");
    assert!(!f.app.editor.diagnostics().contains_key(&alpha_only));
    assert_eq!(f.app.editor.diagnostics()[&shared].len(), 1);
    assert_eq!(f.app.editor.diagnostics()[&shared][0].0.message, "beta");
    // Late notifications from the retired process cannot restore its diagnostics.
    f.app
        .editor
        .handle_publish_diagnostics(alpha, params(&open, None, "late"));
    f.app.editor.handle_language_server_exit(alpha);
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert_eq!(f.messages(), ["beta", "spelling"]);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_clears_workspace_diagnostics_for_unopened_files() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha", "beta"])?;
    f.initialize().await?;
    let alpha = f.server("alpha");
    let beta = f.server("beta");
    let shared = Uri::from(stdx::path::normalize(
        dir.path().join("unopened.lifecycle-test"),
    ));
    let alpha_only = Uri::from(stdx::path::normalize(
        dir.path().join("alpha.lifecycle-test"),
    ));
    for (id, name) in [(alpha, "alpha"), (beta, "beta")] {
        f.app
            .editor
            .handle_publish_diagnostics(id, params(&shared, None, name));
    }
    f.app
        .editor
        .handle_publish_diagnostics(alpha, params(&alpha_only, None, "alpha"));

    let document = current_ref!(f.app.editor).1.id();
    assert!(f
        .app
        .editor
        .stop_language_servers(document, &["unknown"])
        .is_err());
    assert!(f.app.editor.language_server_by_id(alpha).is_some());
    assert!(f.app.editor.diagnostics().contains_key(&alpha_only));
    let exits = Arc::new(AtomicUsize::new(0));
    let seen = exits.clone();
    event::register_hook!(move |event: &mut LanguageServerExited<'_>| {
        assert_eq!(event.server_id, alpha);
        assert!(event.editor.language_server_by_id(alpha).is_some());
        assert!(event
            .editor
            .document(document)
            .unwrap()
            .supports_language_server(alpha));
        assert!(event
            .editor
            .diagnostics()
            .values()
            .flatten()
            .all(|(_, provider)| provider.language_server_id() != Some(alpha)));
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    f.app.editor.stop_language_servers(document, &["alpha"])?;

    assert!(f.app.editor.language_server_by_id(alpha).is_none());
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert!(f.app.editor.language_server_by_id(beta).is_some());
    assert!(!f.app.editor.diagnostics().contains_key(&alpha_only));
    assert_eq!(f.app.editor.diagnostics()[&shared].len(), 1);
    assert_eq!(f.app.editor.diagnostics()[&shared][0].0.message, "beta");
    f.app.editor.refresh_language_servers(document);
    assert!(f.app.editor.language_server_by_id(alpha).is_none());
    assert!(current_ref!(f.app.editor)
        .1
        .language_servers()
        .all(|server| server.name() != "alpha"));
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_adapter_delegates_and_keeps_the_exit_status_message() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha"])?;
    f.release()?;
    let (id, call) = f.next().await?;
    f.app.handle_language_server_message(call, id).await;
    let (id, call) = f.next().await?;
    f.app.handle_language_server_message(call, id).await;
    assert_eq!(f.messages(), ["alpha"]);
    event::register_hook!(move |event: &mut LanguageServerExited<'_>| {
        assert_eq!(
            event.editor.get_status().unwrap().0.as_ref(),
            "Language server exited: alpha"
        );
        Ok(())
    });
    f.app
        .handle_language_server_message(notification("exit", Value::Null), id)
        .await;
    assert!(f.app.editor.language_server_by_id(id).is_none());
    assert!(f.app.editor.diagnostics().is_empty());
    assert!(f.messages().is_empty());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_setup_sends_each_document_notification_once() -> anyhow::Result<()> {
    let first_dir = tempfile::tempdir()?;
    let second_dir = tempfile::tempdir()?;
    let mut first = Fixture::new(first_dir.path(), &["alpha"])?;
    let mut second = Fixture::new(second_dir.path(), &["alpha"])?;
    for fixture in [&mut first, &mut second] {
        fixture.initialize().await?;
        let (view, doc) = current!(fixture.app.editor);
        let id = doc.id();
        let uri = doc.identifier().uri;
        let transaction = Transaction::insert(doc.text(), doc.selection(view.id), "x".into());
        assert!(view::editing::apply(doc, &transaction, view.id));
        fixture.app.editor.new_file(view::editor::Action::Replace);
        assert!(fixture.app.editor.close_document(id, true).is_ok());
        // Application shutdown only flushes outgoing messages. Await the fixture's
        // response explicitly so its log includes all preceding notifications.
        fixture
            .app
            .editor
            .language_server_by_id(fixture.server("alpha"))
            .unwrap()
            .shutdown()
            .await?;
        assert!(fixture.app.close().await.is_empty());
        let messages: Vec<Value> = read_log(&fixture.logs[0])?;
        for method in [
            "textDocument/didOpen",
            "textDocument/didChange",
            "textDocument/didClose",
        ] {
            assert_eq!(
                messages
                    .iter()
                    .filter(|message| message["method"] == method
                        && message["params"]["textDocument"]["uri"] == uri.as_str())
                    .count(),
                1,
                "{method}"
            );
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn document_synchronization_precedes_early_change_and_close_observers() -> anyhow::Result<()>
{
    event::register_event::<DocumentDidChange>();
    event::register_event::<DocumentDidClose>();
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        for server in event.doc.language_servers() {
            server.notify::<ObserverMarker>(json!({"phase": "changed"}));
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        for server in event.doc.language_servers() {
            server.notify::<ObserverMarker>(json!({"phase": "closed"}));
        }
        Ok(())
    });
    let dir = tempfile::tempdir()?;
    let mut fixture = Fixture::new(dir.path(), &["alpha"])?;
    fixture.initialize().await?;
    let (view, doc) = current!(fixture.app.editor);
    let id = doc.id();
    let transaction = Transaction::insert(doc.text(), doc.selection(view.id), "x".into());
    assert!(view::editing::apply(doc, &transaction, view.id));
    assert!(fixture.app.editor.close_document(id, true).is_ok());
    // A response is a transport barrier for all earlier notifications.
    fixture
        .app
        .editor
        .language_server_by_id(fixture.server("alpha"))
        .unwrap()
        .shutdown()
        .await?;
    let messages: Vec<Value> = read_log(&fixture.logs[0])?;
    for (synchronization, phase) in [
        ("textDocument/didChange", "changed"),
        ("textDocument/didClose", "closed"),
    ] {
        let sync = messages
            .iter()
            .position(|message| message["method"] == synchronization)
            .unwrap();
        let observer = messages
            .iter()
            .position(|message| {
                message["method"] == "test/observer" && message["params"]["phase"] == phase
            })
            .unwrap();
        assert!(
            sync < observer,
            "{synchronization} must precede {phase} observers"
        );
    }
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn documents_open_once_with_current_text_after_editor_initialization() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha"])?;
    f.release()?;
    let (server, call) = f.next().await?;
    assert!(matches!(&call, Call::Notification(n) if n.method == "initialized"));
    let second = dir.path().join("second.lifecycle-test");
    std::fs::write(&second, "second\n")?;
    f.app.editor.open(&second, view::editor::Action::Replace)?;
    let (view, doc) = current!(f.app.editor);
    let transaction = Transaction::insert(doc.text(), doc.selection(view.id), "changed ".into());
    assert!(view::editing::apply(doc, &transaction, view.id));
    let uri = doc.identifier().uri;
    let text = doc.text().to_string();
    let version = doc.version();
    f.app.handle_language_server_message(call, server).await;
    // Repeated lifecycle delivery is idempotent, including configuration and observers.
    f.app.editor.handle_language_server_initialized(server);
    f.app
        .editor
        .language_server_by_id(server)
        .unwrap()
        .shutdown()
        .await?;
    let messages: Vec<Value> = read_log(&f.logs[0])?;
    let opens: Vec<_> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m["method"] == "textDocument/didOpen"
                && m["params"]["textDocument"]["uri"] == json!(uri)
        })
        .collect();
    assert_eq!(opens.len(), 1);
    assert_eq!(opens[0].1["params"]["textDocument"]["text"], text);
    assert_eq!(opens[0].1["params"]["textDocument"]["version"], version);
    let configs: Vec<_> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m["method"] == "workspace/didChangeConfiguration")
        .collect();
    assert_eq!(configs.len(), 1);
    assert!(configs[0].0 < opens[0].0);
    assert!(!messages
        .iter()
        .any(|m| m["method"] == "textDocument/didChange"));
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_attachments_follow_renames_and_discard_closed_documents() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha"])?;
    let original = current_ref!(f.app.editor).1.id();
    f.release()?;
    let (server, call) = f.next().await?;
    assert!(matches!(&call, Call::Notification(n) if n.method == "initialized"));
    let second = dir.path().join("closed.lifecycle-test");
    std::fs::write(&second, "closed\n")?;
    let closed = f.app.editor.open(&second, view::editor::Action::Replace)?;
    f.app
        .editor
        .close_document(closed, true)
        .map_err(|_| anyhow::anyhow!("close failed"))?;
    let renamed = dir.path().join("renamed.lifecycle-test");
    f.app.editor.set_doc_path(original, &renamed);
    let uri = f.app.editor.document(original).unwrap().identifier().uri;
    f.app.handle_language_server_message(call, server).await;
    f.app
        .editor
        .language_server_by_id(server)
        .unwrap()
        .shutdown()
        .await?;
    let messages: Vec<Value> = read_log(&f.logs[0])?;
    let opens: Vec<_> = messages
        .iter()
        .filter(|m| m["method"] == "textDocument/didOpen")
        .collect();
    assert_eq!(opens.len(), 1);
    assert_eq!(opens[0]["params"]["textDocument"]["uri"], json!(uri));
    assert!(!messages
        .iter()
        .any(|m| m["method"] == "textDocument/didClose"));
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn language_changes_reopen_documents_on_reused_servers() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut fixture = Fixture::new(dir.path(), &["alpha"])?;
    fixture.initialize().await?;
    let server = fixture.server("alpha");
    let doc = current_ref!(fixture.app.editor).1.id();
    let uri = fixture.uri();
    fixture
        .app
        .editor
        .handle_publish_diagnostics(server, params(&uri, Some(0), "previous language"));
    assert_eq!(fixture.messages(), ["previous language"]);
    fixture.app.editor.set_document_language(doc, None)?;
    fixture
        .app
        .editor
        .set_document_language(doc, Some("lifecycle-test"))?;
    assert_eq!(fixture.server("alpha"), server);
    assert!(fixture.messages().is_empty());
    assert!(!fixture.app.editor.diagnostics().contains_key(&uri));
    // Await a response to flush all preceding notifications into the server's log.
    fixture
        .app
        .editor
        .language_server_by_id(server)
        .unwrap()
        .shutdown()
        .await?;
    let messages = read_log(&fixture.logs[0])?;
    let lifecycle: Vec<_> = messages
        .iter()
        .filter(|message| {
            matches!(
                message["method"].as_str(),
                Some("textDocument/didOpen" | "textDocument/didClose")
            )
        })
        .collect();
    assert_eq!(lifecycle.len(), 3);
    assert_eq!(lifecycle[0]["method"], "textDocument/didOpen");
    assert_eq!(lifecycle[1]["method"], "textDocument/didClose");
    assert_eq!(lifecycle[2]["method"], "textDocument/didOpen");
    assert_eq!(
        lifecycle[2]["params"]["textDocument"],
        lifecycle[0]["params"]["textDocument"]
    );
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn progress_tokens_are_scoped_to_servers_and_removed_on_stop() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha", "beta"])?;
    f.initialize().await?;
    let alpha = f.server("alpha");
    let beta = f.server("beta");
    for server in [alpha, beta] {
        f.app
            .handle_language_server_message(
                notification(
                    "$/progress",
                    json!({
                        "token": "shared", "value": {"kind": "begin", "title": "Indexing"}
                    }),
                ),
                server,
            )
            .await;
    }
    f.app
        .handle_language_server_message(
            notification(
                "$/progress",
                json!({
                    "token": "second", "value": {"kind": "begin", "title": "Checking"}
                }),
            ),
            alpha,
        )
        .await;
    for _ in 0..2 {
        f.app
            .handle_language_server_message(
                notification(
                    "$/progress",
                    json!({
                        "token": "shared", "value": {"kind": "end"}
                    }),
                ),
                alpha,
            )
            .await;
    }
    assert!(f.app.editor.language_server_is_progressing(alpha));
    assert!(f.app.editor.language_server_is_progressing(beta));
    let document = current_ref!(f.app.editor).1.id();
    f.app.editor.stop_language_servers(document, &["alpha"])?;
    assert!(!f.app.editor.language_server_is_progressing(alpha));
    assert!(f.app.editor.language_server_is_progressing(beta));
    // Notifications already in transit cannot restore a stopped server's progress.
    f.app
        .handle_language_server_message(
            notification(
                "$/progress",
                json!({
                    "token": "late", "value": {"kind": "begin", "title": "Late"}
                }),
            ),
            alpha,
        )
        .await;
    assert!(!f.app.editor.language_server_is_progressing(alpha));
    f.app
        .handle_language_server_message(notification("exit", Value::Null), beta)
        .await;
    assert_eq!(f.app.editor.progressing_language_servers().count(), 0);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

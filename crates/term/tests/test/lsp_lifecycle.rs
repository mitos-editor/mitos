use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use editor_core::{diagnostic::DiagnosticProvider, Transaction, Uri};
use lsp_client::{lsp, Call, Notification};
use serde_json::{json, Value};
use view::{current, current_ref, events::LanguageServerExited};

use super::helpers::lsp::Fixture;

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
            let messages: Vec<Value> = std::fs::read_to_string(&f.logs[0])?
                .split_inclusive('\n')
                .filter(|line| line.ends_with('\n'))
                .map(serde_json::from_str)
                .collect::<Result<_, _>>()?;
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
    assert!(f.app.editor.diagnostics.is_empty());
    f.initialize().await?;
    f.app
        .editor
        .handle_publish_diagnostics(id, params(&uri, Some(0), "current"));
    assert_eq!(f.messages(), ["current"]);
    let (view, doc) = current!(f.app.editor);
    let transaction = Transaction::insert(doc.text(), doc.selection(view.id), "x".into());
    assert!(doc.apply(&transaction, view.id));
    let version = doc.version();
    for stale in [version - 1, version + 1] {
        f.app
            .editor
            .handle_publish_diagnostics(id, params(&uri, Some(stale), "stale"));
        assert_eq!(f.messages(), ["current"]);
        assert_eq!(f.app.editor.diagnostics[&uri][0].0.message, "current");
    }
    let mut invalid = params(&uri, None, "invalid URI");
    invalid.uri = "https://example.test/diagnostic".parse()?;
    f.app.editor.handle_publish_diagnostics(id, invalid);
    assert_eq!(f.app.editor.diagnostics.len(), 1);
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
    assert!(f.app.editor.diagnostics.is_empty());
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
            .diagnostics
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
    assert!(!f.app.editor.diagnostics.contains_key(&alpha_only));
    assert_eq!(f.app.editor.diagnostics[&shared].len(), 1);
    assert_eq!(f.app.editor.diagnostics[&shared][0].0.message, "beta");
    f.app.editor.handle_language_server_exit(alpha);
    assert_eq!(exits.load(Ordering::SeqCst), 1);
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
    assert!(f.app.editor.diagnostics.is_empty());
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
        assert!(doc.apply(&transaction, view.id));
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
        let messages: Vec<Value> = std::fs::read_to_string(&fixture.logs[0])?
            .split_inclusive('\n')
            .filter(|line| line.ends_with('\n'))
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
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

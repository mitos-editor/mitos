use std::{path::Path, time::Duration};

use ::diagnostics::DiagnosticProvider;
use anyhow::Context as _;
use editor_core::{Transaction, Uri};
use lsp_client::LanguageServerId;
use serde_json::{json, Value};
use term::application::TestApplication;
use tokio::sync::mpsc;
use view::{
    callbacks::EditorCallback,
    current, current_ref,
    editor::Action,
    features::diagnostics::pull::{self, PullDiagnosticsHandler},
    DocumentId,
};

use super::helpers::lsp::{self, Gate, ServerConfig};
use super::helpers::{test_config, AppBuilder};

struct Fixture {
    app: TestApplication,
    callbacks: mpsc::Receiver<(bool, EditorCallback)>,
    logs: Vec<std::path::PathBuf>,
    gate: Gate,
}

impl Fixture {
    fn new(directory: &Path, word: &str, names: &[&str], inter_file: bool) -> anyhow::Result<Self> {
        let gate = Gate::new(directory.join("initialize-ready"));
        let mut servers = String::new();
        let mut logs = Vec::new();
        for name in names {
            let related = lsp::log_path(directory, name).with_extension("related.json");
            std::fs::write(&related, "{}")?;
            let mut server = ServerConfig::feature(name, "--diagnostics", directory)
                .initialize_gate(&gate)
                .arg("--related-diagnostics")
                .arg(related);
            if inter_file {
                server = server.arg("--inter-file");
            }
            servers.push_str(&server.toml());
            logs.push(lsp::log_path(directory, name));
        }
        let loader = lsp::syntax_loader("pull-diagnostic-test", "pull-test", names, &servers);
        let mut config = test_config();
        config.editor.lsp.enable = true;
        let (tx, callbacks) = mpsc::channel(64);
        let app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(loader)
            .with_handler_setup(move |handlers, _| {
                handlers.pull_diagnostics = PullDiagnosticsHandler::new(
                    super::helpers::callbacks::bounded_sender(&tx, |blocking, callback| {
                        (blocking, callback)
                    }),
                );
            })
            .build()?;
        let mut fixture = Self {
            app,
            callbacks,
            logs,
            gate,
        };
        fixture.open(&directory.join("document.pull-test"), word)?;
        Ok(fixture)
    }

    fn open(&mut self, path: &Path, word: &str) -> anyhow::Result<DocumentId> {
        std::fs::write(path, format!("😀 {word}\n"))?;
        Ok(self.app.editor.open(path, Action::Replace)?)
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        self.gate.release()?;
        lsp::initialize(&mut self.app, self.logs.len()).await
    }

    /// Debounce callbacks start work. Keep publication/retry callbacks under the
    /// test's control so state can change after the server has already replied.
    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (scheduling, callback) =
                    self.callbacks.recv().await.expect("callback queue closed");
                if scheduling {
                    callback(&mut self.app.editor);
                } else {
                    return callback;
                }
            }
        })
        .await
        .context("expected a queued diagnostic response or retry")
    }

    async fn publish(&mut self) -> anyhow::Result<()> {
        let callback = self.response().await?;
        callback(&mut self.app.editor);
        Ok(())
    }

    fn request(&mut self) {
        let id = current_ref!(self.app.editor).1.id();
        pull::request_document_diagnostics(&mut self.app.editor, id);
    }

    fn edit(&mut self, word: &str) {
        let (view, doc) = current!(self.app.editor);
        let change = Transaction::change(
            doc.text(),
            [(2, doc.text().len_chars() - 1, Some(word.into()))].into_iter(),
        );
        view::editing::apply(doc, &change, view.id).unwrap();
    }

    fn server(&self, name: &str) -> LanguageServerId {
        current_ref!(self.app.editor)
            .1
            .language_servers()
            .find(|server| server.name() == name)
            .unwrap()
            .id()
    }

    fn messages(&self) -> Vec<String> {
        let mut messages: Vec<_> = current_ref!(self.app.editor)
            .1
            .diagnostics()
            .iter()
            .map(|d| d.message.to_string())
            .collect();
        messages.sort();
        messages
    }

    fn requests(&self, server: usize) -> anyhow::Result<Vec<Value>> {
        std::fs::read_to_string(&self.logs[server])?
            .lines()
            .map(|line| Ok(serde_json::from_str(line)?))
            .collect()
    }

    fn related(&self, server: usize, uri: &Uri, report: Value) -> anyhow::Result<()> {
        let uri = uri.to_url().unwrap().to_string();
        self.related_reports(server, json!({uri: report}))
    }

    fn related_reports(&self, server: usize, reports: Value) -> anyhow::Result<()> {
        std::fs::write(
            self.logs[server].with_extension("related.json"),
            serde_json::to_vec(&reports)?,
        )?;
        Ok(())
    }
}

fn related_report(result_id: Option<&str>, message: Option<&str>) -> Value {
    let items: Vec<_> = message
        .map(|message| {
            json!({
                "range": {"start": {"line": 0, "character": 3},
                    "end": {"line": 0, "character": 7}},
                "severity": 2, "message": message
            })
        })
        .into_iter()
        .collect();
    let mut report = json!({"kind": "full", "items": items});
    if let Some(result_id) = result_id {
        report["resultId"] = json!(result_id);
    }
    report
}

#[tokio::test(flavor = "multi_thread")]
async fn related_reports_update_unopened_files_even_when_the_requested_report_is_unchanged(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    let uri = Uri::from(stdx::path::normalize(
        dir.path().join("related file.pull-test"),
    ));
    f.related(
        0,
        &uri,
        related_report(Some("related:1"), Some("related error")),
    )?;
    f.initialize().await?;
    f.publish().await?;
    assert_eq!(
        f.app.editor.diagnostics()[&uri][0].0.message,
        "related error"
    );
    assert_eq!(f.app.editor.documents().count(), 1);

    f.related(
        0,
        &uri,
        related_report(Some("related:2"), Some("updated error")),
    )?;
    f.request();
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 original"]);
    assert_eq!(
        f.app.editor.diagnostics()[&uri][0].0.message,
        "updated error"
    );

    f.related(
        0,
        &uri,
        json!({"kind": "unchanged", "resultId": "related:2"}),
    )?;
    f.request();
    f.publish().await?;
    assert_eq!(
        f.app.editor.diagnostics()[&uri][0].0.message,
        "updated error"
    );

    f.related(0, &uri, related_report(None, None))?;
    f.request();
    f.publish().await?;
    assert!(f.app.editor.diagnostics()[&uri].is_empty());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn related_result_ids_follow_the_reported_uri_and_are_cleared_when_absent(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    let source = current_ref!(f.app.editor).1.id();
    let path = dir.path().join("related.pull-test");
    let uri = Uri::from(stdx::path::normalize(&path));
    // The fake server returns unchanged when this ID is sent for the related file.
    f.related(
        0,
        &uri,
        related_report(Some("alpha:0"), Some("cached error")),
    )?;
    f.initialize().await?;
    f.publish().await?;
    f.related_reports(0, json!({}))?;
    let related = f.open(&path, "other")?;
    f.publish().await?;
    assert_eq!(
        f.requests(0)?.last().unwrap()["previousResultId"],
        "alpha:0"
    );
    assert_eq!(f.messages(), ["cached error"]);
    let diagnostic = &current_ref!(f.app.editor).1.diagnostics()[0];
    assert_eq!((diagnostic.range.start, diagnostic.range.end), (2, 6));

    f.app.editor.switch(source, Action::Replace);
    f.related(0, &uri, related_report(None, Some("no result id")))?;
    f.request();
    f.publish().await?;
    assert_eq!(
        f.requests(0)?.last().unwrap()["previousResultId"],
        "alpha:0"
    );
    f.app.editor.switch(related, Action::Replace);
    assert_eq!(f.messages(), ["no result id"]);
    f.related_reports(0, json!({}))?;
    f.request();
    f.publish().await?;
    assert!(f.requests(0)?.last().unwrap()["previousResultId"].is_null());
    assert_eq!(f.messages(), ["alpha: 😀 other"]);

    f.app
        .editor
        .set_document_language(related, Some("pull-diagnostic-test"))?;
    f.publish().await?;
    assert!(f.requests(0)?.last().unwrap()["previousResultId"].is_null());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn related_reports_preserve_other_providers() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha", "beta"], false)?;
    let uri = Uri::from(stdx::path::normalize(dir.path().join("related.pull-test")));
    for (server, name) in [(0, "alpha"), (1, "beta")] {
        f.related(server, &uri, related_report(Some(name), Some(name)))?;
    }
    f.initialize().await?;
    f.publish().await?;
    f.publish().await?;
    assert_eq!(f.app.editor.diagnostics()[&uri].len(), 2);

    f.related(0, &uri, related_report(None, None))?;
    f.related(1, &uri, json!({"kind": "unchanged", "resultId": "beta"}))?;
    f.request();
    f.publish().await?;
    f.publish().await?;
    let diagnostics = &f.app.editor.diagnostics()[&uri];
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].0.message, "beta");
    assert!(matches!(&diagnostics[0].1, DiagnosticProvider::Lsp {
        identifier: Some(id), ..
    } if id.as_ref() == "beta"));
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_and_related_reports_follow_request_order() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    f.initialize().await?;
    f.publish().await?;
    let source = current_ref!(f.app.editor).1.id();
    let related = f.open(&dir.path().join("related.pull-test"), "other")?;
    f.publish().await?;
    let uri = current_ref!(f.app.editor).1.uri().unwrap();

    f.app.editor.switch(source, Action::Replace);
    f.related(0, &uri, related_report(Some("stale"), Some("stale error")))?;
    f.request();
    let old = f.response().await?;
    f.related_reports(0, json!({}))?;
    f.app.editor.switch(related, Action::Replace);
    f.request();
    f.publish().await?;
    old(&mut f.app.editor);
    assert_eq!(f.messages(), ["alpha: 😀 other"]);
    f.request();
    f.publish().await?;
    assert_eq!(
        f.requests(0)?.last().unwrap()["previousResultId"],
        "alpha:0"
    );
    // A newer related report must also win over an older direct response.
    f.request();
    let old = f.response().await?;
    f.app.editor.switch(source, Action::Replace);
    f.related(0, &uri, related_report(Some("newer"), Some("newer error")))?;
    f.request();
    f.publish().await?;
    old(&mut f.app.editor);
    f.app.editor.switch(related, Action::Replace);
    assert_eq!(f.messages(), ["newer error"]);
    f.related_reports(0, json!({}))?;
    f.request();
    f.publish().await?;
    assert_eq!(f.requests(0)?.last().unwrap()["previousResultId"], "newer");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn related_reports_skip_unsupported_uris_and_uncached_unchanged_reports() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    let path = dir.path().join("related.pull-test");
    let uri = Uri::from(stdx::path::normalize(&path));
    let valid = Uri::from(stdx::path::normalize(dir.path().join("valid.pull-test")));
    f.related_reports(0, json!({
        (uri.to_url().unwrap().to_string()): {"kind": "unchanged", "resultId": "alpha:0"},
        (valid.to_url().unwrap().to_string()): related_report(Some("valid"), Some("valid error")),
        "https://example.com/unsupported": related_report(None, Some("unsupported"))
    }))?;
    f.initialize().await?;
    f.publish().await?;
    assert_eq!(
        f.app.editor.diagnostics()[&valid][0].0.message,
        "valid error"
    );
    assert!(!f.app.editor.diagnostics().contains_key(&uri));

    f.related_reports(0, json!({}))?;
    f.open(&path, "other")?;
    f.publish().await?;
    assert!(f.requests(0)?.last().unwrap()["previousResultId"].is_null());
    assert_eq!(f.messages(), ["alpha: 😀 other"]);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn related_unchanged_reports_cannot_reuse_a_discarded_result_id() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    let source = current_ref!(f.app.editor).1.id();
    let path = dir.path().join("related.pull-test");
    let uri = Uri::from(stdx::path::normalize(&path));
    f.related(0, &uri, related_report(Some("old"), Some("old error")))?;
    f.initialize().await?;
    f.publish().await?;

    f.related(
        0,
        &uri,
        related_report(Some("discarded"), Some("discarded error")),
    )?;
    f.request();
    let stale = f.response().await?;
    f.related_reports(0, json!({}))?;
    let related = f.open(&path, "other")?;
    let direct = f.response().await?;
    stale(&mut f.app.editor);
    assert_eq!(f.messages(), ["old error"]);

    f.app.editor.switch(source, Action::Replace);
    f.related(
        0,
        &uri,
        json!({"kind": "unchanged", "resultId": "discarded"}),
    )?;
    f.request();
    f.publish().await?;
    direct(&mut f.app.editor);
    f.app.editor.switch(related, Action::Replace);
    assert_eq!(f.messages(), ["alpha: 😀 other"]);
    f.related_reports(0, json!({}))?;
    f.request();
    f.publish().await?;
    assert_eq!(
        f.requests(0)?.last().unwrap()["previousResultId"],
        "alpha:0"
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_related_reports_validate_the_target_document_lifecycle() -> anyhow::Result<()> {
    for mutation in ["edit", "rename", "language", "close"] {
        let dir = tempfile::tempdir()?;
        let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
        f.initialize().await?;
        f.publish().await?;
        let source = current_ref!(f.app.editor).1.id();
        let related = f.open(&dir.path().join("related.pull-test"), "other")?;
        f.publish().await?;
        let uri = current_ref!(f.app.editor).1.uri().unwrap();
        f.app.editor.switch(source, Action::Replace);
        f.related(0, &uri, related_report(Some("stale"), Some("stale error")))?;
        f.request();
        let old = f.response().await?;
        f.app.editor.switch(related, Action::Replace);
        match mutation {
            "edit" => f.edit("edited"),
            "rename" => f
                .app
                .editor
                .set_doc_path(related, &dir.path().join("renamed.pull-test")),
            "language" => f
                .app
                .editor
                .set_document_language(related, Some("pull-diagnostic-test"))?,
            "close" => assert!(f.app.editor.close_document(related, true).is_ok()),
            _ => unreachable!(),
        }
        let before = f.app.editor.diagnostics().get(&uri).cloned();
        old(&mut f.app.editor);
        assert_eq!(
            f.app.editor.diagnostics().get(&uri),
            before.as_ref(),
            "{mutation}"
        );
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_related_reports_cannot_publish_into_a_newly_opened_document() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    let path = dir.path().join("related.pull-test");
    let uri = Uri::from(stdx::path::normalize(&path));
    f.related(0, &uri, related_report(Some("stale"), Some("stale error")))?;
    f.initialize().await?;
    let old = f.response().await?;
    f.related_reports(0, json!({}))?;
    f.open(&path, "other")?;
    old(&mut f.app.editor);
    assert!(!f.app.editor.diagnostics().contains_key(&uri));
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 other"]);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn full_and_unchanged_reports_preserve_provider_ids_and_utf16_ranges() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    f.initialize().await?;
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 original"]);
    let diagnostic = &current_ref!(f.app.editor).1.diagnostics()[0];
    assert_eq!((diagnostic.range.start, diagnostic.range.end), (2, 10));
    assert!(
        matches!(&diagnostic.provider, DiagnosticProvider::Lsp { identifier: Some(id), .. } if id.as_ref() == "alpha")
    );
    f.request();
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 original"]);
    let requests = f.requests(0)?;
    // Upstream lsp-types serializes an absent result ID as JSON null.
    assert!(requests[0]
        .get("previousResultId")
        .is_none_or(Value::is_null));
    assert_eq!(requests[1]["previousResultId"], "alpha:0");
    assert_eq!(requests[1]["identifier"], "alpha");

    f.edit("no-id");
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 no-id"]);
    f.request();
    f.publish().await?;
    assert!(f
        .requests(0)?
        .last()
        .unwrap()
        .get("previousResultId")
        .is_none_or(Value::is_null));
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_refresh_does_not_cancel_other_providers_or_overwrite_their_results(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha", "beta"], false)?;
    f.initialize().await?;
    let first = f.response().await?;
    let second = f.response().await?;
    let alpha = f.server("alpha");
    pull::request_all_document_diagnostics_for_language_server(&mut f.app.editor, alpha);
    first(&mut f.app.editor);
    second(&mut f.app.editor);
    assert_eq!(f.messages(), ["beta: 😀 original"]);
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 original", "beta: 😀 original"]);
    assert_eq!(f.requests(1)?.len(), 1);
    assert!(
        f.requests(0)?[1]
            .get("previousResultId")
            .is_none_or(Value::is_null),
        "discarded responses must not advance result IDs"
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn events_and_responses_stay_with_simultaneous_editors() -> anyhow::Result<()> {
    let first_dir = tempfile::tempdir()?;
    let second_dir = tempfile::tempdir()?;
    let mut first = Fixture::new(first_dir.path(), "first", &["alpha"], false)?;
    let mut second = Fixture::new(second_dir.path(), "second", &["alpha"], false)?;
    assert_eq!(
        current_ref!(first.app.editor).1.id(),
        current_ref!(second.app.editor).1.id()
    );
    for f in [&mut first, &mut second] {
        f.initialize().await?;
        f.publish().await?;
    }
    first.edit("updated");
    second.edit("another");
    first.publish().await?;
    second.publish().await?;
    assert_eq!(first.messages(), ["alpha: 😀 updated"]);
    assert_eq!(second.messages(), ["alpha: 😀 another"]);
    assert!(first.app.close().await.is_empty());
    assert!(second.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_reports_cannot_publish_after_edits_renames_or_closure() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    f.initialize().await?;
    let old = f.response().await?;
    f.edit("updated");
    old(&mut f.app.editor);
    assert!(f.messages().is_empty());
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 updated"]);

    f.edit("renamed");
    let old = f.response().await?;
    let old_uri = current_ref!(f.app.editor).1.uri().unwrap();
    f.app.editor.set_doc_path(
        current_ref!(f.app.editor).1.id(),
        &dir.path().join("renamed.pull-test"),
    );
    let after_rename = f.messages();
    old(&mut f.app.editor);
    // The queued report must change neither the coordinated rename's diagnostic
    // state nor the old URI's cache.
    assert_eq!(f.messages(), after_rename);
    assert_eq!(
        f.app.editor.diagnostics()[&old_uri][0].0.message,
        "alpha: 😀 updated"
    );
    // Closing before publication must not insert an entry into the global cache.
    let id = f.open(&dir.path().join("closed.pull-test"), "closed")?;
    let old = f.response().await?;
    let uri = current_ref!(f.app.editor).1.uri().unwrap();
    assert!(f.app.editor.close_document(id, true).is_ok());
    old(&mut f.app.editor);
    assert!(!f.app.editor.diagnostics().contains_key(&uri));
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn server_retry_uses_the_owner_and_a_superseded_retry_cannot_restart_work(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "retry", &["alpha"], false)?;
    f.initialize().await?;
    f.publish().await?; // apply the server-requested retry
    f.publish().await?; // apply its successful report
    assert_eq!(f.messages(), ["alpha: 😀 retry"]);
    assert_eq!(f.requests(0)?.len(), 2);

    f.edit("retry-again");
    let retry = f.response().await?;
    f.request(); // supersede the retry while it waits in the callback queue
    retry(&mut f.app.editor);
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 retry-again"]);
    assert_eq!(f.requests(0)?.len(), 4);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inter_file_dependencies_refresh_other_open_documents() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "first", &["alpha"], true)?;
    f.initialize().await?;
    f.publish().await?;
    let first_uri = current_ref!(f.app.editor).1.url().unwrap().to_string();
    f.open(&dir.path().join("second.pull-test"), "second")?;
    f.publish().await?;
    f.edit("updated");
    f.publish().await?; // the document's 250 ms debounce
    f.publish().await?; // inter-file refresh, first document
    f.publish().await?; // inter-file refresh, second document (order is irrelevant)
    assert_eq!(f.messages(), ["alpha: 😀 updated"]);
    assert_eq!(
        f.requests(0)?
            .iter()
            .filter(|r| r["textDocument"]["uri"] == first_uri)
            .count(),
        2
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn exited_servers_cannot_publish_queued_reports() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    f.initialize().await?;
    let response = f.response().await?;
    let server_id = f.server("alpha");
    f.app.editor.handle_language_server_exit(server_id);
    response(&mut f.app.editor);
    assert!(f.messages().is_empty());
    assert!(f.app.editor.diagnostics().is_empty());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_refresh_request_targets_only_the_requesting_provider() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha", "beta"], false)?;
    f.initialize().await?;
    f.publish().await?;
    f.publish().await?;
    let alpha = f.server("alpha");
    let call = serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0", "id": "refresh", "method": "workspace/diagnostic/refresh"
    }))?;
    f.app.handle_language_server_message(call, alpha).await;
    f.publish().await?;
    assert_eq!(f.messages(), ["alpha: 😀 original", "beta: 😀 original"]);
    assert_eq!(f.requests(0)?.len(), 2);
    assert_eq!(f.requests(1)?.len(), 1);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn progress_completion_refreshes_diagnostics_while_a_prompt_is_open() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "original", &["alpha"], false)?;
    f.initialize().await?;
    f.publish().await?;
    assert_eq!(f.requests(0)?.len(), 1);
    super::helpers::keys(&mut f.app, ":").await?;
    f.app.editor.set_status("prompt status");
    let server = f.server("alpha");
    for value in [
        serde_json::json!({"kind": "begin", "title": "Indexing"}),
        serde_json::json!({"kind": "report", "percentage": 50}),
        serde_json::json!({"kind": "end"}),
    ] {
        let completed = value["kind"] == "end";
        let call = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": {"token": "indexing", "value": value}
        }))?;
        f.app.handle_language_server_message(call, server).await;
        assert_eq!(
            f.app.editor.language_server_is_progressing(server),
            !completed
        );
        assert_eq!(
            f.app.editor.get_status().unwrap().0.as_ref(),
            "prompt status"
        );
    }
    f.publish().await?;
    assert_eq!(f.requests(0)?.len(), 2);
    assert_eq!(f.messages(), ["alpha: 😀 original"]);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

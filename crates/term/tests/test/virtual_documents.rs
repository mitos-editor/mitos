//! Server-provided content keeps URI/provider identity and a read-only lifetime.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use editor_core::{Selection, Transaction};
use lsp_client::{lsp, LanguageServerId};
use serde_json::{json, Value};
use term::application::TestApplication;
use tokio::sync::{mpsc, oneshot};
use view::{
    callbacks::EditorCallback,
    current, current_ref,
    editor::Action,
    features::navigation::{self, NavigationKind},
    DocumentId,
};

use super::helpers::{
    callbacks,
    lsp::{self as fixture_lsp, Gate, ServerConfig},
    test_config, AppBuilder,
};

const URI: &str = "test-source:/generated.virtual-test";
type OpenResult = oneshot::Receiver<anyhow::Result<DocumentId>>;

struct Fixture {
    app: TestApplication,
    callbacks: mpsc::UnboundedReceiver<(bool, EditorCallback)>,
    servers: BTreeMap<String, LanguageServerId>,
    directory: PathBuf,
    origin: DocumentId,
}

impl Fixture {
    async fn new(directory: &Path, names: &[&str], args: &[&str]) -> anyhow::Result<Self> {
        let gate = Gate::new(directory.join("initialize"));
        let mut configuration = String::new();
        for name in names {
            let content = directory.join(format!("{name}.content"));
            std::fs::write(&content, format!("😀 generated {name}\n"))?;
            let mut server = ServerConfig::feature(name, "--virtual-documents", directory)
                .initialize_gate(&gate)
                .arg("--content-file")
                .arg(&content)
                .arg("--virtual-uri")
                .arg(URI);
            for arg in args {
                server = server.arg(arg);
            }
            configuration.push_str(&server.toml());
        }
        let (sender, callbacks) = callbacks::deferred(|blocking, callback| (blocking, callback));
        let mut config = test_config();
        config.editor.lsp.enable = true;
        config.editor.idle_timeout = editor_config::Config::default().idle_timeout;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(fixture_lsp::syntax_loader(
                "virtual-test",
                "virtual-test",
                names,
                &configuration,
            ))
            .with_handler_setup(move |handlers, _| {
                // Hold only content and navigation publications. Startup work
                // keeps its normal frontend queue and cannot act as a response.
                let sender = sender(handlers.redraw().clone());
                handlers.virtual_documents =
                    view::features::virtual_documents::VirtualDocumentsHandler::new(sender.clone());
                handlers.navigation = view::features::navigation::NavigationHandler::new(sender);
            })
            .build()?;
        let path = directory.join("origin.virtual-test");
        std::fs::write(&path, "😀 origin\n")?;
        let origin = app.editor.open(&path, Action::Replace)?;
        gate.release()?;
        fixture_lsp::initialize(&mut app, names.len()).await?;
        let servers = current_ref!(app.editor)
            .1
            .language_servers()
            .map(|server| (server.name().into(), server.id()))
            .collect();
        Ok(Self {
            app,
            callbacks,
            servers,
            directory: directory.into(),
            origin,
        })
    }

    fn into_ui(self) -> TestApplication {
        let Self {
            app, mut callbacks, ..
        } = self;
        let jobs = app.job_sender();
        tokio::spawn(async move {
            while let Some((_, callback)) = callbacks.recv().await {
                jobs.send_callback(term::job::Callback::Editor(callback))
                    .await;
            }
        });
        app
    }

    fn server(&self, name: &str) -> LanguageServerId {
        self.servers[name]
    }

    fn request(
        &mut self,
        name: &str,
        uri: &str,
        selection: Option<lsp::Range>,
        action: Action,
    ) -> anyhow::Result<OpenResult> {
        let (sender, result) = oneshot::channel();
        self.app.editor.request_language_server_document(
            self.server(name),
            uri.parse()?,
            selection,
            action,
            move |_, result| {
                let _ = sender.send(result);
            },
        );
        Ok(result)
    }

    async fn finish(&mut self, mut result: OpenResult) -> anyhow::Result<DocumentId> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    result = &mut result => return result.context("open callback dropped")?,
                    callback = self.callbacks.recv() => {
                        let (_, callback) = callback.context("editor callback queue closed")?;
                        callback(&mut self.app.editor);
                    }
                }
            }
        })
        .await?
    }

    async fn open(&mut self, name: &str, action: Action) -> anyhow::Result<DocumentId> {
        let result = self.request(name, URI, None, action)?;
        self.finish(result).await
    }

    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        callbacks::response(&mut self.app, &mut self.callbacks).await
    }

    async fn server_call(&mut self, method: &str) -> anyhow::Result<()> {
        let (server, call) = tokio::time::timeout(
            Duration::from_secs(5),
            self.app.editor.next_language_server_message(),
        )
        .await?
        .context("LSP message stream closed")?;
        anyhow::ensure!(
            matches!(&call, lsp_client::Call::MethodCall(call) if call.method == method)
        );
        self.app.handle_language_server_message(call, server).await;
        Ok(())
    }

    async fn call(
        &mut self,
        name: &str,
        id: &str,
        method: &str,
        params: Value,
    ) -> anyhow::Result<()> {
        let call = lsp_client::Call::MethodCall(serde_json::from_value(json!({
            "jsonrpc":"2.0", "id":id, "method":method, "params":params
        }))?);
        self.app
            .handle_language_server_message(call, self.server(name))
            .await;
        Ok(())
    }

    fn messages(&self, name: &str) -> anyhow::Result<Vec<Value>> {
        fixture_lsp::read_log(&fixture_lsp::log_path(&self.directory, name))
    }

    async fn reply(&self, name: &str, id: &str) -> anyhow::Result<Value> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(reply) = self
                    .messages(name)?
                    .into_iter()
                    .find(|message| message["id"] == id && !message["method"].is_string())
                {
                    return Ok(reply);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?
    }

    fn content(&self, name: &str, text: &str) -> anyhow::Result<()> {
        std::fs::write(self.directory.join(format!("{name}.content")), text)?;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn show_document_opens_reuses_and_protects_server_content() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut f = Fixture::new(directory.path(), &["alpha"], &["--show-document"]).await?;
    f.server_call("window/showDocument").await?;
    f.response().await?(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    let document = doc.id();
    assert_eq!(doc.text(), "😀 generated alpha\n");
    assert_eq!(doc.url().unwrap().as_str(), URI);
    assert_eq!(doc.uri().unwrap().to_url().unwrap().as_str(), URI);
    assert!(doc.path().is_none() && doc.source_path().is_none());
    assert!(doc.is_readonly() && doc.is_content_readonly());
    assert_eq!(doc.selection(view.id).primary().head, 2);
    assert_eq!(
        f.reply("alpha", "virtual-show").await?["result"]["success"],
        true
    );
    let export = directory.path().join("export.rs");
    let (view, doc) = current!(f.app.editor);
    assert!(doc.save(Some(export.clone()), true).is_err());
    let edit = Transaction::insert(doc.text(), &Selection::point(0), "edited".into());
    assert!(view::editing::apply(doc, &edit, view.id).is_err());
    assert!(!export.exists());
    assert_eq!(f.open("alpha", Action::Replace).await?, document);
    let messages = f.messages("alpha")?;
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["method"] == "workspace/textDocumentContent")
            .count(),
        1
    );
    let initialize = messages
        .iter()
        .find(|message| message["method"] == "initialize")
        .unwrap();
    assert_eq!(
        initialize["params"]["capabilities"]["workspace"]["textDocumentContent"]
            ["dynamicRegistration"],
        false
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn equal_uris_from_different_content_providers_remain_distinct() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut f = Fixture::new(directory.path(), &["alpha", "beta"], &[]).await?;
    let alpha = f.open("alpha", Action::Load).await?;
    let beta = f.open("beta", Action::Load).await?;
    assert_ne!(alpha, beta);
    assert_eq!(current_ref!(f.app.editor).1.id(), f.origin);
    for (name, document) in [("alpha", alpha), ("beta", beta)] {
        let doc = f.app.editor.document(document).unwrap();
        assert_eq!(doc.url().unwrap().as_str(), URI);
        assert_eq!(doc.text().to_string(), format!("😀 generated {name}\n"));
        assert_eq!(
            f.app
                .editor
                .language_server_document(f.server(name), &URI.parse()?),
            Some(document)
        );
        assert_eq!(f.open(name, Action::Load).await?, document);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn content_refresh_updates_in_place_and_rejects_queued_older_generations(
) -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut f = Fixture::new(directory.path(), &["alpha"], &[]).await?;
    let document = f.open("alpha", Action::Replace).await?;
    f.content("alpha", "😀 old generation\n")?;
    f.call(
        "alpha",
        "content-refresh",
        "workspace/textDocumentContent/refresh",
        json!({"uri":URI}),
    )
    .await?;
    let old = f.response().await?;
    f.content("alpha", "😀 newest generation\n")?;
    f.app
        .editor
        .refresh_language_server_document(f.server("alpha"), URI.parse()?);
    old(&mut f.app.editor);
    assert_eq!(
        f.app.editor.document(document).unwrap().text(),
        "😀 generated alpha\n"
    );
    f.response().await?(&mut f.app.editor);
    let doc = f.app.editor.document(document).unwrap();
    assert_eq!(doc.text(), "😀 newest generation\n");
    assert!(!doc.is_modified() && doc.is_content_readonly());
    assert_eq!(
        f.app
            .editor
            .language_server_document(f.server("alpha"), &URI.parse()?),
        Some(document)
    );
    let reply = f.reply("alpha", "content-refresh").await?;
    assert!(reply["result"].is_null() && reply.get("error").is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unsupported_content_schemes_fail_without_fetching_or_changing_focus() -> anyhow::Result<()>
{
    let directory = tempfile::tempdir()?;
    let mut f = Fixture::new(directory.path(), &["alpha"], &[]).await?;
    f.call(
        "alpha",
        "unsupported",
        "window/showDocument",
        json!({"uri":"other-source:/generated.rs"}),
    )
    .await?;
    assert_eq!(
        f.reply("alpha", "unsupported").await?["result"]["success"],
        false
    );
    assert_eq!(current_ref!(f.app.editor).1.id(), f.origin);
    assert!(!f
        .messages("alpha")?
        .iter()
        .any(|message| message["method"] == "workspace/textDocumentContent"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delayed_content_cannot_take_focus_after_the_user_switches_buffers() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut f = Fixture::new(directory.path(), &["alpha"], &[]).await?;
    let result = f.request("alpha", URI, None, Action::Replace)?;
    let response = f.response().await?;
    let other = f.app.editor.new_file(Action::Replace);
    response(&mut f.app.editor);
    assert!(tokio::time::timeout(Duration::from_secs(5), result)
        .await??
        .is_err());
    assert_eq!(current_ref!(f.app.editor).1.id(), other);
    assert_eq!(
        f.app
            .editor
            .language_server_document(f.server("alpha"), &URI.parse()?),
        None
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_content_document_discards_its_queued_refresh() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut f = Fixture::new(directory.path(), &["alpha"], &[]).await?;
    let document = f.open("alpha", Action::Replace).await?;
    f.content("alpha", "replacement\n")?;
    f.app
        .editor
        .refresh_language_server_document(f.server("alpha"), URI.parse()?);
    let response = f.response().await?;
    f.app.editor.switch(f.origin, Action::Replace);
    assert!(f.app.editor.close_document(document, true).is_ok());
    response(&mut f.app.editor);
    assert!(f.app.editor.document(document).is_none());
    assert_eq!(
        f.app
            .editor
            .language_server_document(f.server("alpha"), &URI.parse()?),
        None
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stopped_content_providers_cannot_publish_queued_documents() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut f = Fixture::new(directory.path(), &["alpha"], &[]).await?;
    let result = f.request("alpha", URI, None, Action::Load)?;
    let response = f.response().await?;
    f.app.editor.handle_language_server_exit(f.server("alpha"));
    response(&mut f.app.editor);
    assert!(tokio::time::timeout(Duration::from_secs(5), result)
        .await??
        .is_err());
    assert_eq!(current_ref!(f.app.editor).1.id(), f.origin);
    assert_eq!(
        f.app
            .editor
            .language_server_document(f.server("alpha"), &URI.parse()?),
        None
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_preserves_virtual_target_ownership_and_position_encoding() -> anyhow::Result<()>
{
    let directory = tempfile::tempdir()?;
    let mut f = Fixture::new(directory.path(), &["alpha"], &[]).await?;
    navigation::request(&mut f.app.editor, NavigationKind::Definition);
    f.response().await?(&mut f.app.editor);
    let update = navigation::next_update(&f.app.editor).context("missing navigation update")?;
    let locations = update
        .resolve(&mut f.app.editor)
        .context("stale navigation update")?;
    assert_eq!(locations.len(), 1);
    let target = &locations[0];
    assert_eq!(target.uri.to_url().unwrap().as_str(), URI);
    assert_eq!(target.server_id, Some(f.server("alpha")));
    assert_eq!(target.offset_encoding, lsp_client::OffsetEncoding::Utf16);
    let result = f.request("alpha", URI, Some(target.range), Action::Replace)?;
    let document = f.finish(result).await?;
    let (view, doc) = current_ref!(f.app.editor);
    assert_eq!(doc.id(), document);
    assert_eq!(doc.selection(view.id).primary().head, 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "runtime-grammars"),
    ignore = "requires the compiled Rust grammar"
)]
async fn syntax_workspace_symbols_use_document_identity_for_virtual_sources() -> anyhow::Result<()>
{
    use std::{
        ops::ControlFlow,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };
    use view::features::syntax_symbols::{Target, WorkspaceSymbols};

    let directory = tempfile::tempdir()?;
    let mut f = Fixture::new(directory.path(), &["alpha"], &[]).await?;
    f.content("alpha", "fn generated_virtual() {}\n")?;
    let result = f.request("alpha", "test-source:/generated.rs", None, Action::Replace)?;
    let document = f.finish(result).await?;
    f.app.editor.ensure_syntax(document);
    assert!(f.app.editor.document(document).unwrap().syntax().is_some());
    let symbols = WorkspaceSymbols::new(&f.app.editor, document, search::Options::default());
    let found = Arc::new(AtomicBool::new(false));
    let observed = found.clone();
    symbols
        .query(
            &mut f.app.editor,
            "generated_virtual",
            move |symbol| {
                assert!(matches!(&symbol.target, Target::Document(id) if *id == document));
                // Picker identity used to unwrap a local path for every document URI.
                assert_eq!(symbol.key().0, Err(document));
                observed.store(true, Ordering::Relaxed);
                ControlFlow::Break(())
            },
            || false,
        )
        .await?;
    assert!(found.load(Ordering::Relaxed));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn lazy_workspace_symbols_preserve_virtual_provider_and_opaque_data() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut f = Fixture::new(directory.path(), &["alpha", "beta"], &[]).await?;
    let session = f.app.editor.workspace_symbols(f.origin)?;
    let mut task = session.query(&f.app.editor, "match".into())?;
    let mut providers = Vec::new();
    for _ in 0..2 {
        let items = tokio::time::timeout(Duration::from_secs(5), task.next_batch())
            .await?
            .context("workspace symbol response")??;
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.uri.to_url().unwrap().as_str(), URI);
        assert!(item.location(&f.app.editor).is_none());
        let update =
            tokio::time::timeout(Duration::from_secs(5), item.resolve(&f.app.editor)?.run())
                .await??
                .context("symbol resolution")?;
        update.publish(&f.app.editor).context("current symbol")?;
        let location = item.location(&f.app.editor).unwrap();
        assert_eq!(location.server_id, item.provider());
        assert_eq!(location.offset_encoding, lsp_client::OffsetEncoding::Utf16);
        assert_eq!(location.range.start.character, 3);
        providers.push(location.server_id.unwrap());
    }
    assert_ne!(providers[0], providers[1]);
    for name in ["alpha", "beta"] {
        let messages = f.messages(name)?;
        let resolve = messages
            .iter()
            .find(|message| message["method"] == "workspaceSymbol/resolve")
            .context("resolve request")?;
        assert_eq!(resolve["params"]["location"], json!({"uri":URI}));
        assert_eq!(
            resolve["params"]["data"],
            json!({"provider":name,"symbol":"match","token":[42,{"raw":true}]})
        );
        assert!(!messages
            .iter()
            .any(|message| message["method"] == "workspace/textDocumentContent"));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn lazy_workspace_preview_loads_only_the_selected_virtual_document_and_navigates(
) -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let content = Gate::new(directory.path().join("preview-content-ready"));
    let f = Fixture::new(
        directory.path(),
        &["alpha"],
        &["--content-gate", content.path().to_str().unwrap()],
    )
    .await?;
    let origin = f.origin;
    let server = f.server("alpha");
    let mut app = f.into_ui();
    super::helpers::keys(&mut app, "<space>S").await?;
    // Preview loads are background work and may finish after the idle frame.
    let uri = URI.parse()?;
    assert!(app.editor.language_server_document(server, &uri).is_none());
    content.release()?;
    let document = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(document) = app.editor.language_server_document(server, &uri) {
                return anyhow::Ok(document);
            }
            super::helpers::keys(&mut app, "").await?;
        }
    })
    .await
    .context("selected preview content")??;
    assert_eq!(current_ref!(app.editor).1.id(), origin);
    assert_eq!(
        app.editor.document(document).unwrap().text(),
        "😀 generated alpha\n"
    );
    assert_eq!(
        app.editor
            .language_server_document(server, &format!("{URI}?other").parse()?),
        None
    );
    let messages = fixture_lsp::read_log(&fixture_lsp::log_path(directory.path(), "alpha"))?;
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["method"] == "workspaceSymbol/resolve")
            .count(),
        1
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["method"] == "workspace/textDocumentContent")
            .count(),
        1
    );
    super::helpers::keys(&mut app, "<ret>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.id(), document);
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        2
    );
    assert!(doc.is_readonly());
    assert!(app.close().await.is_empty());
    Ok(())
}

async fn hidden_workspace_picker(
    directory: &Path,
    extra: &[&str],
) -> anyhow::Result<(TestApplication, DocumentId, LanguageServerId)> {
    let gate = Gate::new(directory.join("symbol-query-ready"));
    let mut args = vec!["--symbol-query-gate", gate.path().to_str().unwrap()];
    args.extend_from_slice(extra);
    let f = Fixture::new(directory, &["alpha"], &args).await?;
    let origin = f.origin;
    let server = f.server("alpha");
    let mut app = f.into_ui();
    super::helpers::keys(&mut app, "<space>S<C-t>").await?;
    gate.release()?;
    super::helpers::keys(&mut app, "match").await?;
    app.editor
        .replace_quicklist(vec![view::quicklist::QuicklistEntry {
            target: view::quicklist::QuicklistTarget::Document(origin),
            position: view::quicklist::QuicklistPosition::None,
        }]);
    Ok((app, origin, server))
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_virtual_quicklist_export_loads_only_matches_without_changing_focus(
) -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, origin, server) = hidden_workspace_picker(directory.path(), &[]).await?;
    assert_eq!(
        app.editor.language_server_document(server, &URI.parse()?),
        None
    );
    assert!(
        !fixture_lsp::read_log(&fixture_lsp::log_path(directory.path(), "alpha"))?
            .iter()
            .any(|message| message["method"] == "workspace/textDocumentContent")
    );
    super::helpers::keys(&mut app, "<C-q>").await?;
    let document = app
        .editor
        .language_server_document(server, &URI.parse()?)
        .context("exported virtual document")?;
    assert_eq!(current_ref!(app.editor).1.id(), origin);
    let entries = app.editor.quicklist.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].target,
        view::quicklist::QuicklistTarget::Document(document)
    );
    assert!(
        matches!(entries[0].position, view::quicklist::QuicklistPosition::LspRange {
        range, offset_encoding: lsp_client::OffsetEncoding::Utf16
    } if range.start.character == 3 && range.end.character == 12)
    );
    assert_eq!(
        app.editor
            .language_server_document(server, &format!("{URI}?other").parse()?),
        None
    );
    let messages = fixture_lsp::read_log(&fixture_lsp::log_path(directory.path(), "alpha"))?;
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["method"] == "workspaceSymbol/resolve")
            .count(),
        1
    );
    let loads: Vec<_> = messages
        .iter()
        .filter(|message| message["method"] == "workspace/textDocumentContent")
        .collect();
    assert_eq!(loads.len(), 1);
    assert_eq!(loads[0]["params"]["uri"], URI);
    super::helpers::keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn canceled_cold_virtual_export_preserves_the_previous_quicklist() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let content = Gate::new(directory.path().join("content-ready"));
    let (mut app, origin, server) = hidden_workspace_picker(
        directory.path(),
        &["--content-gate", content.path().to_str().unwrap()],
    )
    .await?;
    let previous = app.editor.quicklist.entries().to_vec();
    super::helpers::keys(&mut app, "<C-q>").await?;
    assert_eq!(app.editor.quicklist.entries(), previous);
    super::helpers::keys(&mut app, "<esc>").await?;
    content.release()?;
    super::helpers::keys(&mut app, "").await?;
    assert_eq!(app.editor.quicklist.entries(), previous);
    assert_eq!(
        app.editor.language_server_document(server, &URI.parse()?),
        None
    );
    assert_eq!(current_ref!(app.editor).1.id(), origin);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_cold_virtual_export_preserves_the_previous_quicklist() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, origin, server) =
        hidden_workspace_picker(directory.path(), &["--fail-virtual-content"]).await?;
    let previous = app.editor.quicklist.entries().to_vec();
    super::helpers::keys(&mut app, "<C-q>").await?;
    assert_eq!(app.editor.quicklist.entries(), previous);
    assert_eq!(
        app.editor.language_server_document(server, &URI.parse()?),
        None
    );
    assert_eq!(current_ref!(app.editor).1.id(), origin);
    assert!(app
        .editor
        .get_status()
        .is_some_and(|(message, _)| message.contains("test content failure")));
    super::helpers::keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_picker_refreshes_when_its_virtual_origin_changes() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut f = Fixture::new(
        directory.path(),
        &["alpha"],
        &["--changing-virtual-content"],
    )
    .await?;
    let document = f.open("alpha", Action::Replace).await?;
    let server = f.server("alpha");
    let mut app = f.into_ui();
    super::helpers::keys(&mut app, "<space>S<C-t>").await?;
    let before: String = app
        .terminal_backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(before.contains("alpha:match:1"), "{before}");
    app.editor
        .refresh_language_server_document(server, URI.parse()?);
    super::helpers::keys(&mut app, "").await?;
    assert_eq!(
        app.editor.document(document).unwrap().text(),
        "😀 generated alpha 2\n"
    );
    let after: String = app
        .terminal_backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(after.contains("alpha:match:2"), "{after}");
    assert!(!after.contains("alpha:match:1"), "{after}");
    super::helpers::keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

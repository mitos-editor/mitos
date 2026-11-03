//! Shared navigation semantics through the production editor dispatcher.
use crate::support;
use anyhow::Context as _;
use arc_swap::ArcSwap;
use editor_core::{Range, Selection, Transaction};
use lsp_client::{
    test_support::{self as lsp, Gate, ServerConfig},
    OffsetEncoding,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::mpsc;
use view::{
    callbacks::EditorCallback,
    current, current_ref,
    editor::Action,
    features::navigation::{self, Location, NavigationHandler, NavigationKind, NavigationUpdate},
    Editor,
};

struct Fixture {
    editor: Editor,
    config: Arc<ArcSwap<editor_config::Config>>,
    callbacks: mpsc::UnboundedReceiver<EditorCallback>,
    logs: Vec<PathBuf>,
    _directory: tempfile::TempDir,
}

fn syntax_loader(config: &str) -> language::syntax::Loader {
    language::syntax::Loader::new(
        toml::from_str(config).unwrap(),
        loader::syntax::Resources::default(),
    )
    .unwrap()
}

impl Fixture {
    async fn new(dir: &Path) -> anyhow::Result<Self> {
        Self::with_servers(dir, &[("alpha", &[])]).await
    }

    async fn with_servers(dir: &Path, servers: &[(&str, &[&str])]) -> anyhow::Result<Self> {
        let gate = Gate::new(dir.join("initialize-ready"));
        let mut configuration = String::new();
        let mut names = Vec::new();
        let mut logs = Vec::new();
        for (name, args) in servers {
            let mut server =
                ServerConfig::feature(name, "--navigation", dir).initialize_gate(&gate);
            for arg in *args {
                server = server.arg(arg);
            }
            configuration.push_str(&server.toml(env!("CARGO_BIN_EXE_mitos-view-test-lsp")));
            names.push(*name);
            logs.push(lsp::log_path(dir, name));
        }
        let (sender, callbacks) = support::callback_channel();
        let mut f = support::Fixture::with_handler_setup(
            "",
            &lsp::language_config("navigation-test", "navigation-test", &names, &configuration),
            loader::syntax::Resources::default(),
            |config| {
                config.lsp.enable = true;
                config.lsp.document_highlight.auto_trigger = false;
            },
            move |handlers, _| handlers.navigation = NavigationHandler::new(sender),
        )?;
        let path = dir.join("document.navigation-test");
        std::fs::write(&path, "😀 foo foo\n")?;
        f.editor.open(&path, Action::Replace)?;
        gate.release()?;
        tokio::time::timeout(Duration::from_secs(10), async {
            for _ in servers {
                let (server, call) = f
                    .editor
                    .next_language_server_message()
                    .await
                    .context("server closed during initialization")?;
                anyhow::ensure!(
                    matches!(&call, lsp_client::Call::Notification(n) if n.method == "initialized")
                );
                assert!(f
                    .editor
                    .handle_language_server_message(call, server)
                    .await
                    .is_none());
            }
            anyhow::Ok(())
        })
        .await??;
        Ok(Self {
            editor: f.editor,
            config: f.config,
            callbacks,
            logs,
            _directory: f.dir,
        })
    }

    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        tokio::time::timeout(Duration::from_secs(5), self.callbacks.recv())
            .await?
            .context("navigation queue closed")
    }
    async fn update(&mut self, kind: NavigationKind) -> anyhow::Result<NavigationUpdate> {
        navigation::request(&mut self.editor, kind);
        self.response().await?(&mut self.editor);
        navigation::next_update(&self.editor).context("navigation result missing")
    }
    async fn locations(&mut self, kind: NavigationKind) -> anyhow::Result<Vec<Location>> {
        self.update(kind)
            .await?
            .resolve(&mut self.editor)
            .context("navigation result invalid")
    }
    fn select(&mut self, cursor: usize) {
        let (view, doc) = current!(self.editor);
        doc.set_selection(view.id, Selection::point(cursor));
    }
    fn replace(&mut self, text: &str) {
        let (view, doc) = current!(self.editor);
        let change = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        view::editing::apply(doc, &change, view.id).unwrap();
    }
    async fn wait_message(&self, method: &str) -> anyhow::Result<serde_json::Value> {
        lsp::wait_message(&self.logs[0], method).await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_normalizes_location_shapes_and_preserves_target_ranges() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    for kind in [
        NavigationKind::Definition,
        NavigationKind::Declaration,
        NavigationKind::TypeDefinition,
        NavigationKind::Implementation,
        NavigationKind::References,
    ] {
        let locations = f.locations(kind).await?;
        assert_eq!(locations.len(), 1, "{kind:?}");
        let location = &locations[0];
        assert_eq!(location.offset_encoding, OffsetEncoding::Utf16);
        let doc = current_ref!(f.editor).1;
        assert_eq!(location.uri, doc.uri().unwrap());
        assert_eq!(
            lsp_client::util::lsp_range_to_range(
                doc.text(),
                location.range,
                location.offset_encoding
            ),
            Some(Range::new(2, 5)),
            "{kind:?}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_aggregates_servers_with_their_position_encodings_and_keeps_partial_results(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_servers(
        dir.path(),
        &[
            ("alpha", &[]),
            ("beta", &["--utf-8"]),
            ("gamma", &["--navigation-error"]),
        ],
    )
    .await?;
    let locations = f.locations(NavigationKind::Definition).await?;
    assert_eq!(locations.len(), 2);
    assert!(locations
        .iter()
        .any(|location| location.offset_encoding == OffsetEncoding::Utf8));
    assert!(locations
        .iter()
        .any(|location| location.offset_encoding == OffsetEncoding::Utf16));
    for location in locations {
        assert_eq!(
            lsp_client::util::lsp_range_to_range(
                current_ref!(f.editor).1.text(),
                location.range,
                location.offset_encoding
            ),
            Some(Range::new(2, 5))
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn references_use_the_configured_declaration_policy() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let mut config = (*f.editor.config()).clone();
    config.lsp.references.include_declaration = false;
    let old = f.config.load_full();
    f.config.store(std::sync::Arc::new(config));
    f.editor.refresh_config(&old);
    f.locations(NavigationKind::References).await?;
    let request = f.wait_message("textDocument/references").await?;
    assert_eq!(request["params"]["context"]["includeDeclaration"], false);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_rejects_edits_before_publication_and_presentation() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    navigation::request(&mut f.editor, NavigationKind::Definition);
    let callback = f.response().await?;
    f.replace("😀 bar bar\n");
    callback(&mut f.editor);
    assert!(navigation::next_update(&f.editor).is_none());
    let update = f.update(NavigationKind::References).await?;
    f.replace("😀 new new\n");
    assert!(update.resolve(&mut f.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_rejects_language_reload_and_detached_servers() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let update = f.update(NavigationKind::Definition).await?;
    let loader = syntax_loader(
        r#"
        [[language]]
        name = "navigation-test"
        scope = "source.navigation-test"
        file-types = ["navigation-test"]
        roots = []
        language-servers = ["alpha"]
    "#,
    );
    f.editor
        .apply_language_config(loader, f.editor.theme.clone())?;
    assert!(update.resolve(&mut f.editor).is_none());
    let update = f.update(NavigationKind::References).await?;
    current!(f.editor).1.remove_language_server_by_name("alpha");
    assert!(update.resolve(&mut f.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_cancels_inflight_requests_on_focus_loss() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_servers(dir.path(), &[("alpha", &["--hold-navigation"])]).await?;
    navigation::request(&mut f.editor, NavigationKind::Definition);
    let request = f.wait_message("textDocument/definition").await?;
    f.editor.new_file(Action::Replace);
    let cancel = f.wait_message("$/cancelRequest").await?;
    assert_eq!(cancel["params"]["id"], request["id"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_and_reference_selection_supersede_older_requests() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let old = f.update(NavigationKind::Definition).await?;
    navigation::request(&mut f.editor, NavigationKind::SelectReferences);
    let selection = f.response().await?;
    assert!(old.resolve(&mut f.editor).is_none());
    navigation::request(&mut f.editor, NavigationKind::References);
    let response = f.response().await?;
    selection(&mut f.editor);
    assert_eq!(
        current_ref!(f.editor)
            .1
            .selection(current_ref!(f.editor).0.id)
            .len(),
        1
    );
    response(&mut f.editor);
    assert!(navigation::next_update(&f.editor)
        .unwrap()
        .resolve(&mut f.editor)
        .is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn select_references_preserves_the_primary_cursor_and_uses_only_the_first_server(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_servers(dir.path(), &[("alpha", &[]), ("beta", &[])]).await?;
    f.select(7);
    navigation::request(&mut f.editor, NavigationKind::SelectReferences);
    f.response().await?(&mut f.editor);
    let (view, doc) = current_ref!(f.editor);
    let selection = doc.selection(view.id);
    assert_eq!(selection.ranges(), &[Range::new(2, 5), Range::new(6, 9)]);
    assert_eq!(selection.primary(), Range::new(6, 9));
    assert!(!std::fs::read_to_string(&f.logs[1])?.contains("textDocument/documentHighlight"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn select_references_handles_empty_responses_and_clamps_out_of_bounds_ranges(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    for text in ["empty\n", "invalid\n"] {
        f.replace(text);
        f.select(0);
        let original = current_ref!(f.editor)
            .1
            .selection(current_ref!(f.editor).0.id)
            .clone();
        navigation::request(&mut f.editor, NavigationKind::SelectReferences);
        f.response().await?(&mut f.editor);
        let (view, doc) = current_ref!(f.editor);
        let expected = if text == "empty\n" {
            original
        } else {
            Selection::point(doc.text().len_chars())
        };
        assert_eq!(doc.selection(view.id), &expected);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_navigation_reports_a_feature_specific_error_only_while_current() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    f.replace("empty\n");
    let update = f.update(NavigationKind::Definition).await?;
    assert!(update.resolve(&mut f.editor).is_none());
    assert_eq!(f.editor.get_status().unwrap().0, "No definition found.");
    let update = f.update(NavigationKind::References).await?;
    f.editor.status_msg = None;
    f.select(1);
    assert!(update.resolve(&mut f.editor).is_none());
    assert!(f.editor.get_status().is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_does_not_interrupt_a_new_editing_mode() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let original = current_ref!(f.editor)
        .1
        .selection(current_ref!(f.editor).0.id)
        .clone();
    navigation::request(&mut f.editor, NavigationKind::SelectReferences);
    let callback = f.response().await?;
    f.editor.set_mode(view::document::Mode::Insert);
    callback(&mut f.editor);
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.selection(view.id), &original);
    f.editor.enter_normal_mode();
    let update = f.update(NavigationKind::Definition).await?;
    f.editor.set_mode(view::document::Mode::Insert);
    f.editor.enter_normal_mode();
    assert!(update.resolve(&mut f.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inbound_dispatch_keeps_error_codes_and_deferred_reply_policy() -> anyhow::Result<()> {
    use serde_json::{json, Value};
    use view::handlers::lsp::HostAction;
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let server = current_ref!(f.editor)
        .1
        .language_servers()
        .next()
        .unwrap()
        .id();
    for (id, method, params, code) in [
        ("unknown", "test/unknown", json!({}), -32601),
        ("malformed", "workspace/applyEdit", json!({}), -32700),
    ] {
        let call = serde_json::from_value(
            json!({"jsonrpc":"2.0", "id":id,"method":method,"params":params}),
        )?;
        assert!(f
            .editor
            .handle_language_server_message(call, server)
            .await
            .is_none());
        let reply = wait_reply(&f.logs[0], id).await?;
        assert_eq!(reply["error"]["code"], code);
        assert!(reply.get("result").is_none());
    }
    let call = serde_json::from_value(
        json!({"jsonrpc":"2.0", "id":"status", "method":"window/showMessageRequest", "params":{"type":3,"message":"server status"}}),
    )?;
    assert!(f
        .editor
        .handle_language_server_message(call, server)
        .await
        .is_none());
    assert_eq!(
        wait_reply(&f.logs[0], "status").await?["result"],
        Value::Null
    );
    assert_eq!(f.editor.get_status().unwrap().0, "server status");

    for (id, complete) in [("chosen", true), ("replaced", false)] {
        let call = serde_json::from_value(
            json!({"jsonrpc":"2.0", "id":id, "method":"window/showMessageRequest", "params":{"type":3,"message":"choose","actions":[{"title":"yes"}]}}),
        )?;
        let Some(HostAction::ShowMessage {
            message,
            actions,
            reply,
        }) = f.editor.handle_language_server_message(call, server).await
        else {
            panic!("choice did not reach the host")
        };
        assert_eq!(message, "choose");
        assert_eq!(actions[0].title, "yes");
        assert!(!lsp::read_log(&f.logs[0])?.iter().any(|m| m["id"] == id));
        if complete {
            reply.complete(&f.editor, Ok(json!(actions[0])));
            assert_eq!(wait_reply(&f.logs[0], id).await?["result"]["title"], "yes");
        } else {
            drop(reply);
        }
    }
    let call = serde_json::from_value(
        json!({"jsonrpc":"2.0", "id":"document", "method":"window/showDocument", "params":{"uri":"file:///target","takeFocus":true}}),
    )?;
    let Some(HostAction::ShowDocument {
        params,
        offset_encoding,
        reply,
    }) = f.editor.handle_language_server_message(call, server).await
    else {
        panic!("document choice did not reach host")
    };
    assert_eq!(params.take_focus, Some(true));
    assert_eq!(offset_encoding, OffsetEncoding::Utf16);
    reply.complete(&f.editor, Ok(json!({"success":false})));
    assert_eq!(
        wait_reply(&f.logs[0], "document").await?["result"]["success"],
        false
    );
    // A later reply is a wire-order barrier: dropping/replacing a prompt stays silent.
    let records = lsp::read_log(&f.logs[0])?;
    assert!(!records.iter().any(|m| m["id"] == "replaced"));
    assert_eq!(records.iter().filter(|m| m["id"] == "chosen").count(), 1);
    let other_dir = tempfile::tempdir()?;
    let mut other = Fixture::new(other_dir.path()).await?;
    let other_id = current_ref!(other.editor)
        .1
        .language_servers()
        .next()
        .unwrap()
        .id();
    assert_eq!(other_id, server, "exercise registry-local ID reuse");
    let call = serde_json::from_value(
        json!({"jsonrpc":"2.0", "id":"foreign", "method":"window/showMessageRequest", "params":{"type":3,"message":"choose","actions":[{"title":"yes"}]}}),
    )?;
    let Some(HostAction::ShowMessage { reply, .. }) =
        f.editor.handle_language_server_message(call, server).await
    else {
        panic!("missing prompt")
    };
    reply.complete(&other.editor, Ok(Value::Null));
    let call = serde_json::from_value(
        json!({"jsonrpc":"2.0", "id":"barrier", "method":"workspace/workspaceFolders"}),
    )?;
    assert!(other
        .editor
        .handle_language_server_message(call, other_id)
        .await
        .is_none());
    wait_reply(&other.logs[0], "barrier").await?;
    assert!(!lsp::read_log(&other.logs[0])?
        .iter()
        .any(|m| m["id"] == "foreign"));
    Ok(())
}

async fn wait_reply(log: &Path, id: &str) -> anyhow::Result<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(reply) = lsp::read_log(log)?.into_iter().find(|m| m["id"] == id) {
                return anyhow::Ok(reply);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?
}

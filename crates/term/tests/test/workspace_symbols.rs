//! Workspace symbol sessions keep their original providers and release canceled requests.
use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};
use anyhow::Context as _;
use std::{path::Path, time::Duration};
use term::application::TestApplication;
use view::{current_ref, editor::Action};

async fn fixture(dir: &Path, args: &[&str]) -> anyhow::Result<TestApplication> {
    let initialize = Gate::new(dir.join("initialize"));
    let mut server =
        ServerConfig::feature("symbols", "--workspace-symbols", dir).initialize_gate(&initialize);
    for arg in args {
        server = server.arg(arg);
    }
    let mut config = test_config();
    // Let the real dynamic-query debounce publish its callbacks before idle.
    config.editor.idle_timeout = editor_config::Config::default().idle_timeout;
    config.editor.lsp.enable = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(lsp::syntax_loader(
            "symbols",
            "symbols",
            &["symbols"],
            &server.toml(),
        ))
        .build()?;
    let path = dir.join("document.symbols");
    std::fs::write(&path, "😀 foo\n")?;
    app.editor.open(&path, Action::Replace)?;
    initialize.release()?;
    lsp::initialize(&mut app, 1).await?;
    Ok(app)
}

#[tokio::test(flavor = "multi_thread")]
async fn symbol_queries_use_the_originating_providers_after_focus_moves() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &[]).await?;
    let document = current_ref!(app.editor).1.id();
    let session = app.editor.workspace_symbols(document)?;
    app.editor.new_file(Action::Replace);
    let mut task = session.query(&app.editor, "foo".into())?;
    let items = tokio::time::timeout(Duration::from_secs(5), task.next_batch())
        .await?
        .context("missing symbols")??;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "symbols:foo");
    assert_eq!(
        items[0].location(&app.editor).unwrap().offset_encoding,
        lsp_client::OffsetEncoding::Utf16
    );
    assert_eq!(
        items[0].location(&app.editor).unwrap().uri,
        app.editor.document(document).unwrap().uri().unwrap()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_symbol_query_cancels_its_server_request() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--hold-symbols"]).await?;
    let document = current_ref!(app.editor).1.id();
    let session = app.editor.workspace_symbols(document)?;
    let task = session.query(&app.editor, "pending".into())?;
    wait_message(dir.path(), "workspace/symbol").await?;
    drop(task);
    wait_message(dir.path(), "$/cancelRequest").await?;
    app.editor.new_file(Action::Replace);
    assert!(app.editor.close_document(document, true).is_ok());
    assert!(!session.is_current(&app.editor));
    assert!(session.query(&app.editor, "closed".into()).is_err());
    Ok(())
}

async fn wait_message(dir: &Path, method: &str) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if lsp::read_log(&lsp::log_path(dir, "symbols"))?
                .iter()
                .any(|message| message["method"] == method)
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?
}

async fn wait_picker_requests(
    app: &mut TestApplication,
    dir: &Path,
    method: &str,
    count: usize,
) -> anyhow::Result<Vec<serde_json::Value>> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let messages: Vec<_> = lsp::read_log(&lsp::log_path(dir, "symbols"))?
                .into_iter()
                .filter(|message| message["method"] == method)
                .collect();
            if messages.len() >= count {
                return Ok(messages);
            }
            // Publishing picker rows and starting their lazy preview require
            // UI updates even when the previous input pump already went idle.
            super::helpers::keys(app, "").await?;
        }
    })
    .await
    .with_context(|| format!("picker did not send {count} {method} requests"))?
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_symbol_picker_cancels_pending_queries() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--hold-symbols"]).await?;
    super::helpers::test_key_sequences(
        &mut app,
        vec![(Some("<space>S"), None), (Some("<esc>"), None)],
        false,
    )
    .await?;
    wait_message(dir.path(), "$/cancelRequest").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_symbol_picker_refreshes_edited_buffers_and_keeps_its_query() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--symbols-from-text"]).await?;
    super::helpers::keys(&mut app, "<space>Sfoo").await?;
    let (view, doc) = view::current!(app.editor);
    let edit = editor_core::Transaction::change(
        doc.text(),
        [(0, doc.text().len_chars(), Some("new\n".into()))].into_iter(),
    );
    view::editing::apply(doc, &edit, view.id).unwrap();
    super::helpers::keys(&mut app, "").await?;
    let screen: String = app
        .terminal_backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("symbols:new:foo"), "{screen}");
    super::helpers::keys(&mut app, "<esc>").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn revisiting_a_picker_query_restarts_its_canceled_request() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--hold-symbols"]).await?;
    super::helpers::test_key_sequences(
        &mut app,
        vec![
            (Some("<space>S"), None),
            (Some("fo<backspace><backspace>"), None),
        ],
        false,
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let queries: Vec<_> = lsp::read_log(&lsp::log_path(dir.path(), "symbols"))?
                .into_iter()
                .filter(|message| message["method"] == "workspace/symbol")
                .collect();
            if queries.len() >= 2 {
                assert_eq!(queries.last().unwrap()["params"]["query"], "");
                return anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await??;
    Ok(())
}

async fn lazy_items(
    app: &mut TestApplication,
    query: &str,
) -> anyhow::Result<(
    view::features::workspace_symbols::WorkspaceSymbolsSession,
    Vec<std::sync::Arc<view::features::workspace_symbols::WorkspaceSymbol>>,
)> {
    let document = current_ref!(app.editor).1.id();
    let session = app.editor.workspace_symbols(document)?;
    let mut task = session.query(&app.editor, query.into())?;
    let items = tokio::time::timeout(Duration::from_secs(5), task.next_batch())
        .await?
        .context("missing lazy symbols")??;
    Ok((session, items))
}

#[tokio::test(flavor = "multi_thread")]
async fn lazy_symbols_resolve_only_on_demand_and_preserve_wire_data() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--lazy-symbols"]).await?;
    let (_session, items) = lazy_items(&mut app, "foo").await?;
    assert_eq!(items.len(), 2);
    assert!(items
        .iter()
        .all(|item| item.location(&app.editor).is_none()));
    let messages = lsp::read_log(&lsp::log_path(dir.path(), "symbols"))?;
    assert!(!messages
        .iter()
        .any(|message| message["method"] == "workspaceSymbol/resolve"));
    let initialize = messages
        .iter()
        .find(|message| message["method"] == "initialize")
        .unwrap();
    assert_eq!(
        initialize["params"]["capabilities"]["workspace"]["symbol"]["resolveSupport"]["properties"],
        serde_json::json!(["location.range"])
    );

    let update = items[0].resolve(&app.editor)?.run().await?.unwrap();
    assert!(
        items[0].location(&app.editor).is_none(),
        "publication must validate the editor"
    );
    let item = update.publish(&app.editor).context("current resolution")?;
    let location = item.location(&app.editor).unwrap();
    assert_eq!(location.offset_encoding, lsp_client::OffsetEncoding::Utf16);
    assert_eq!(location.range.start.character, 3);
    assert!(items[1].location(&app.editor).is_none());
    item.resolve(&app.editor)?
        .run()
        .await?
        .unwrap()
        .publish(&app.editor)
        .unwrap();
    let resolves: Vec<_> = lsp::read_log(&lsp::log_path(dir.path(), "symbols"))?
        .into_iter()
        .filter(|message| message["method"] == "workspaceSymbol/resolve")
        .collect();
    assert_eq!(resolves.len(), 1, "cached locations need no second request");
    assert_eq!(
        resolves[0]["params"]["data"],
        serde_json::json!({"provider":"symbols","query":"foo","token":[1,{"opaque":true}]})
    );
    assert_eq!(
        resolves[0]["params"]["location"],
        serde_json::json!({"uri": current_ref!(app.editor).1.identifier().uri})
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn superseding_a_query_discards_queued_symbol_resolutions() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--lazy-symbols"]).await?;
    let (session, items) = lazy_items(&mut app, "old").await?;
    let update = items[0].resolve(&app.editor)?.run().await?.unwrap();
    let _next = session.query(&app.editor, "new".into())?;
    assert!(update.publish(&app.editor).is_none());
    assert!(items[0].location(&app.editor).is_none());
    assert!(items[0].resolve(&app.editor).is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_and_stopped_providers_discard_queued_symbol_resolutions() -> anyhow::Result<()> {
    for stop in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut app = fixture(dir.path(), &["--lazy-symbols"]).await?;
        let (_session, items) = lazy_items(&mut app, "foo").await?;
        let update = items[0].resolve(&app.editor)?.run().await?.unwrap();
        if stop {
            let document = current_ref!(app.editor).1.id();
            app.editor.stop_language_servers(document, &["symbols"])?;
        } else {
            let (view, doc) = view::current!(app.editor);
            let edit = editor_core::Transaction::insert(
                doc.text(),
                doc.selection(view.id),
                "changed".into(),
            );
            view::editing::apply(doc, &edit, view.id).unwrap();
        }
        assert!(update.publish(&app.editor).is_none());
        assert!(items[0].location(&app.editor).is_none());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_lazy_resolution_cancels_its_server_request() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--lazy-symbols", "--hold-symbol-resolves"]).await?;
    let (_session, items) = lazy_items(&mut app, "foo").await?;
    let request = items[0].resolve(&app.editor)?;
    wait_message(dir.path(), "workspaceSymbol/resolve").await?;
    drop(request);
    wait_message(dir.path(), "$/cancelRequest").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn symbol_picker_resolves_the_selected_preview_and_exports_precise_quicklist_locations(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--lazy-symbols"]).await?;
    super::helpers::keys(&mut app, "<space>S").await?;
    wait_picker_requests(&mut app, dir.path(), "workspaceSymbol/resolve", 1).await?;
    super::helpers::keys(&mut app, "").await?;
    let resolves: Vec<_> = lsp::read_log(&lsp::log_path(dir.path(), "symbols"))?
        .into_iter()
        .filter(|message| message["method"] == "workspaceSymbol/resolve")
        .collect();
    assert_eq!(
        resolves.len(),
        1,
        "only the selected preview resolves automatically"
    );
    super::helpers::keys(&mut app, "<C-q>").await?;
    assert_eq!(app.editor.quicklist.entries().len(), 2);
    for entry in app.editor.quicklist.entries() {
        assert!(
            matches!(entry.position, view::quicklist::QuicklistPosition::LspRange { range, offset_encoding: lsp_client::OffsetEncoding::Utf16 } if range.start.character == 3)
        );
    }
    super::helpers::keys(&mut app, "<ret>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        2,
        "UTF-16 range resolves after the emoji"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_symbols_discard_unsupported_location_schemes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut app = fixture(dir.path(), &["--unsupported-symbols"]).await?;
    let (_session, items) = lazy_items(&mut app, "foo").await?;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "symbols:foo");
    assert!(items[0].location(&app.editor).is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn lazy_symbol_resolutions_keep_their_originating_provider_after_focus_moves(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("initialize"));
    let servers = ["alpha", "beta"]
        .map(|name| {
            ServerConfig::feature(name, "--workspace-symbols", dir.path())
                .arg("--lazy-symbols")
                .initialize_gate(&gate)
                .toml()
        })
        .concat();
    let mut config = test_config();
    config.editor.lsp.enable = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(lsp::syntax_loader(
            "symbols",
            "symbols",
            &["alpha", "beta"],
            &servers,
        ))
        .build()?;
    let path = dir.path().join("document.symbols");
    std::fs::write(&path, "😀 foo\n")?;
    app.editor.open(&path, Action::Replace)?;
    gate.release()?;
    lsp::initialize(&mut app, 2).await?;
    let document = current_ref!(app.editor).1.id();
    let session = app.editor.workspace_symbols(document)?;
    app.editor.new_file(Action::Replace);
    let mut task = session.query(&app.editor, "foo".into())?;
    let mut providers = Vec::new();
    for _ in 0..2 {
        let items = tokio::time::timeout(Duration::from_secs(5), task.next_batch())
            .await?
            .unwrap()?;
        let item = &items[0];
        providers.push(item.provider().unwrap());
        let update = item.resolve(&app.editor)?.run().await?.unwrap();
        assert!(update.publish(&app.editor).is_some());
        assert_eq!(
            item.location(&app.editor).unwrap().uri,
            app.editor.document(document).unwrap().uri().unwrap()
        );
    }
    assert_ne!(providers[0], providers[1]);
    for name in ["alpha", "beta"] {
        let resolves: Vec<_> = lsp::read_log(&lsp::log_path(dir.path(), name))?
            .into_iter()
            .filter(|message| message["method"] == "workspaceSymbol/resolve")
            .collect();
        assert_eq!(resolves.len(), 1);
        assert_eq!(resolves[0]["params"]["data"]["provider"], name);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_symbol_previews_resolve_only_on_accept_and_keep_split_navigation(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("symbol-query-ready"));
    let mut app = fixture(
        dir.path(),
        &[
            "--lazy-symbols",
            "--symbol-query-gate",
            gate.path().to_str().unwrap(),
        ],
    )
    .await?;
    // Hold results until preview is hidden, independently of event-loop timing.
    super::helpers::keys(&mut app, "<space>S<C-t>").await?;
    gate.release()?;
    super::helpers::keys(&mut app, "").await?;
    let messages = lsp::read_log(&lsp::log_path(dir.path(), "symbols"))?;
    assert!(!messages
        .iter()
        .any(|message| message["method"] == "workspaceSymbol/resolve"));
    super::helpers::keys(&mut app, "<C-v>").await?;
    wait_message(dir.path(), "workspaceSymbol/resolve").await?;
    super::helpers::keys(&mut app, "").await?;
    assert_eq!(app.editor.tree.views().count(), 2);
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        2
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_symbol_picker_cancels_lazy_preview_and_quicklist_requests(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("symbol-query"));
    let mut app = fixture(
        dir.path(),
        &[
            "--lazy-symbols",
            "--hold-symbol-resolves",
            "--symbol-query-gate",
            gate.path().to_str().unwrap(),
        ],
    )
    .await?;
    // Results arrive after the first idle pump, as they can under CI load.
    super::helpers::keys(&mut app, "<space>S").await?;
    gate.release()?;
    let preview = wait_picker_requests(&mut app, dir.path(), "workspaceSymbol/resolve", 1).await?;
    assert_eq!(preview.len(), 1, "only the selected preview should resolve");
    super::helpers::keys(&mut app, "<C-q>").await?;
    let resolves = wait_picker_requests(&mut app, dir.path(), "workspaceSymbol/resolve", 3).await?;
    assert_eq!(resolves.len(), 3, "one preview and two quicklist resolves");
    assert_eq!(resolves[0]["id"], preview[0]["id"]);
    super::helpers::keys(&mut app, "<esc>").await?;
    let canceled = wait_picker_requests(&mut app, dir.path(), "$/cancelRequest", 3).await?;
    for resolve in resolves {
        assert!(canceled
            .iter()
            .any(|cancel| cancel["params"]["id"] == resolve["id"]));
    }
    assert!(app.editor.quicklist.entries().is_empty());
    assert!(app.close().await.is_empty());
    Ok(())
}

//! Shared navigation policy, request lifetime, and frontend delivery contracts.
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use editor_core::{Range, Selection, Transaction};
use lsp_client::OffsetEncoding;
use term::application::Application;
use tokio::sync::mpsc;
use view::{
    callbacks::EditorCallback,
    current, current_ref,
    editor::{Action, ConfigEvent},
    features::navigation::{self, Location, NavigationHandler, NavigationKind, NavigationUpdate},
};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config, test_syntax_loader, AppBuilder,
};

struct Fixture {
    app: Application,
    callbacks: mpsc::UnboundedReceiver<EditorCallback>,
    logs: Vec<PathBuf>,
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
            configuration.push_str(&server.toml());
            names.push(*name);
            logs.push(lsp::log_path(dir, name));
        }
        let mut config = test_config();
        config.editor.lsp.enable = true;
        config.editor.lsp.document_highlight.auto_trigger = false;
        let (sender, callbacks) = super::helpers::callbacks::channel();
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(lsp::syntax_loader(
                "navigation-test",
                "navigation-test",
                &names,
                &configuration,
            ))
            .with_handler_setup(move |handlers, _| {
                handlers.navigation = NavigationHandler::new(sender)
            })
            .build()?;
        let path = dir.join("document.navigation-test");
        std::fs::write(&path, "😀 foo foo\n")?;
        app.editor.open(&path, Action::Replace)?;
        gate.release()?;
        lsp::initialize(&mut app, servers.len()).await?;
        Ok(Self {
            app,
            callbacks,
            logs,
        })
    }

    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        tokio::time::timeout(Duration::from_secs(5), self.callbacks.recv())
            .await?
            .context("navigation queue closed")
    }

    async fn update(&mut self, kind: NavigationKind) -> anyhow::Result<NavigationUpdate> {
        navigation::request(&mut self.app.editor, kind);
        self.response().await?(&mut self.app.editor);
        navigation::next_update(&self.app.editor).context("navigation result missing")
    }

    async fn locations(&mut self, kind: NavigationKind) -> anyhow::Result<Vec<Location>> {
        self.update(kind)
            .await?
            .resolve(&mut self.app.editor)
            .context("navigation result invalid")
    }

    fn select(&mut self, cursor: usize) {
        let (view, doc) = current!(self.app.editor);
        doc.set_selection(view.id, Selection::point(cursor));
    }

    fn replace(&mut self, text: &str) {
        let (view, doc) = current!(self.app.editor);
        let change = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        assert!(doc.apply(&change, view.id));
    }

    async fn wait_message(&self, method: &str) -> anyhow::Result<serde_json::Value> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(message) = std::fs::read_to_string(&self.logs[0])
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                    .find(|message| message["method"] == method)
                {
                    return message;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .with_context(|| format!("server did not receive {method}"))
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
        let doc = current_ref!(f.app.editor).1;
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
                current_ref!(f.app.editor).1.text(),
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
    let mut config = (*f.app.editor.config()).clone();
    config.lsp.references.include_declaration = false;
    f.app
        .handle_config_events(ConfigEvent::Update(Box::new(config)));
    f.locations(NavigationKind::References).await?;
    let request = f.wait_message("textDocument/references").await?;
    assert_eq!(request["params"]["context"]["includeDeclaration"], false);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_rejects_edits_before_publication_and_presentation() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    navigation::request(&mut f.app.editor, NavigationKind::Definition);
    let callback = f.response().await?;
    f.replace("😀 bar bar\n");
    callback(&mut f.app.editor);
    assert!(navigation::next_update(&f.app.editor).is_none());
    let update = f.update(NavigationKind::References).await?;
    f.replace("😀 new new\n");
    assert!(update.resolve(&mut f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_rejects_selection_changes_and_views_reused_for_other_documents(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let update = f.update(NavigationKind::Definition).await?;
    f.select(2);
    f.select(0);
    assert!(update.resolve(&mut f.app.editor).is_none());
    let update = f.update(NavigationKind::Definition).await?;
    let document = current_ref!(f.app.editor).1.id();
    f.app.editor.new_file(Action::Replace);
    f.app.editor.switch(document, Action::Replace);
    assert!(update.resolve(&mut f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_rejects_language_reload_and_detached_servers() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let update = f.update(NavigationKind::Definition).await?;
    let loader = test_syntax_loader(Some(
        r#"
        [[language]]
        name = "navigation-test"
        scope = "source.navigation-test"
        file-types = ["navigation-test"]
        roots = []
        language-servers = ["alpha"]
    "#
        .into(),
    ));
    f.app
        .editor
        .apply_language_config(loader, f.app.editor.theme.clone())?;
    assert!(update.resolve(&mut f.app.editor).is_none());
    let update = f.update(NavigationKind::References).await?;
    current!(f.app.editor)
        .1
        .remove_language_server_by_name("alpha");
    assert!(update.resolve(&mut f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_cancels_inflight_requests_on_focus_loss() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_servers(dir.path(), &[("alpha", &["--hold-navigation"])]).await?;
    navigation::request(&mut f.app.editor, NavigationKind::Definition);
    let request = f.wait_message("textDocument/definition").await?;
    f.app.editor.new_file(Action::Replace);
    let cancel = f.wait_message("$/cancelRequest").await?;
    assert_eq!(cancel["params"]["id"], request["id"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_and_reference_selection_supersede_older_requests() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let old = f.update(NavigationKind::Definition).await?;
    navigation::request(&mut f.app.editor, NavigationKind::SelectReferences);
    let selection = f.response().await?;
    assert!(old.resolve(&mut f.app.editor).is_none());
    navigation::request(&mut f.app.editor, NavigationKind::References);
    let response = f.response().await?;
    selection(&mut f.app.editor);
    assert_eq!(
        current_ref!(f.app.editor)
            .1
            .selection(current_ref!(f.app.editor).0.id)
            .len(),
        1
    );
    response(&mut f.app.editor);
    assert!(navigation::next_update(&f.app.editor)
        .unwrap()
        .resolve(&mut f.app.editor)
        .is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn select_references_preserves_the_primary_cursor_and_uses_only_the_first_server(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_servers(dir.path(), &[("alpha", &[]), ("beta", &[])]).await?;
    f.select(7);
    navigation::request(&mut f.app.editor, NavigationKind::SelectReferences);
    f.response().await?(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
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
        let original = current_ref!(f.app.editor)
            .1
            .selection(current_ref!(f.app.editor).0.id)
            .clone();
        navigation::request(&mut f.app.editor, NavigationKind::SelectReferences);
        f.response().await?(&mut f.app.editor);
        let (view, doc) = current_ref!(f.app.editor);
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
    assert!(update.resolve(&mut f.app.editor).is_none());
    assert_eq!(f.app.editor.get_status().unwrap().0, "No definition found.");
    let update = f.update(NavigationKind::References).await?;
    f.app.editor.status_msg = None;
    f.select(1);
    assert!(update.resolve(&mut f.app.editor).is_none());
    assert!(f.app.editor.get_status().is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_does_not_interrupt_a_new_editing_mode() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let original = current_ref!(f.app.editor)
        .1
        .selection(current_ref!(f.app.editor).0.id)
        .clone();
    navigation::request(&mut f.app.editor, NavigationKind::SelectReferences);
    let callback = f.response().await?;
    f.app.editor.mode = view::document::Mode::Insert;
    callback(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    assert_eq!(doc.selection(view.id), &original);
    f.app.editor.enter_normal_mode();
    let update = f.update(NavigationKind::Definition).await?;
    f.app.editor.mode = view::document::Mode::Insert;
    navigation::mode_changed(&mut f.app.editor, view::document::Mode::Normal);
    f.app.editor.enter_normal_mode();
    assert!(update.resolve(&mut f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_rejects_renamed_and_closed_documents() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let update = f.update(NavigationKind::Definition).await?;
    let document = current_ref!(f.app.editor).1.id();
    f.app
        .editor
        .set_doc_path(document, &dir.path().join("renamed.navigation-test"));
    assert!(update.resolve(&mut f.app.editor).is_none());
    let update = f.update(NavigationKind::References).await?;
    f.app.editor.new_file(Action::Replace);
    assert!(f.app.editor.close_document(document, true).is_ok());
    assert!(update.resolve(&mut f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_events_jump_to_the_resolved_symbol() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let update = f.update(NavigationKind::Definition).await?;
    f.app
        .handle_editor_event(view::editor::EditorEvent::Navigation(update))
        .await;
    let (view, doc) = current_ref!(f.app.editor);
    let selection = doc.selection(view.id).primary();
    assert_eq!(selection.from(), 2);
    assert_eq!(selection.to(), 5);
    assert_eq!(selection.cursor(doc.text().slice(..)), 2);
    Ok(())
}

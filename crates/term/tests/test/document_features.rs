use std::{path::Path, time::Duration};

use anyhow::Context as _;
use editor_core::{Selection, Transaction};
use term::application::Application;
use tokio::sync::mpsc;
use view::{
    callbacks::{EditorCallback, EditorCallbackSender},
    current, current_ref,
    editor::{Action, ConfigEvent},
    features::{
        code_lenses::DocumentCodeLensesHandler, document_colors::DocumentColorsHandler,
        document_highlight::DocumentHighlightHandler, document_links::DocumentLinksHandler,
        document_symbols::DocumentSymbolsHandler,
    },
};

use super::helpers::lsp::{self, Gate, ServerConfig};
use super::helpers::{test_config, AppBuilder};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Feature {
    Lenses,
    Colors,
    Highlights,
    Links,
    Symbols,
}

struct Fixture {
    app: Application,
    gates: Vec<Gate>,
    callbacks: mpsc::Receiver<(Feature, bool, EditorCallback)>,
}

fn sender(
    feature: Feature,
    tx: &mpsc::Sender<(Feature, bool, EditorCallback)>,
) -> EditorCallbackSender {
    super::helpers::callbacks::bounded_sender(tx, move |blocking, callback| {
        (feature, blocking, callback)
    })
}

impl Fixture {
    fn new(path: &Path, text: &str) -> anyhow::Result<Self> {
        Self::with_servers(path, text, &["document-feature-test"])
    }

    fn with_servers(path: &Path, text: &str, servers: &[&str]) -> anyhow::Result<Self> {
        Self::with_options(path, text, servers, false)
    }

    fn with_options(
        path: &Path,
        text: &str,
        servers: &[&str],
        resolve_links: bool,
    ) -> anyhow::Result<Self> {
        std::fs::write(path, text)?;
        let mut config = test_config();
        config.editor.lsp.enable = true;
        config.editor.lsp.document_highlight.auto_trigger = true;
        config.editor.breadcrumb.enable = true;
        let gates: Vec<_> = servers
            .iter()
            .map(|name| Gate::new(path.with_extension(format!("{name}.initialize-ready"))))
            .collect();
        let server_config = servers
            .iter()
            .zip(&gates)
            .map(|(name, gate)| {
                let server = ServerConfig::new(name).initialize_gate(gate);
                if resolve_links {
                    server
                        .arg("--resolve-links")
                        .arg("--resolve-link-gate")
                        .arg(path.with_extension("resolve-ready"))
                        .toml()
                } else {
                    server.toml()
                }
            })
            .collect::<String>();
        let loader = lsp::syntax_loader(
            "document-feature-test",
            "feature-test",
            servers,
            &server_config,
        );
        let (tx, callbacks) = mpsc::channel(64);
        // Use independent queues so publication can be held while the editor changes.
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(loader)
            .with_handler_setup(move |handlers, _| {
                handlers.document_colors = DocumentColorsHandler::new(sender(Feature::Colors, &tx));
                handlers.code_lenses = DocumentCodeLensesHandler::new(sender(Feature::Lenses, &tx));
                handlers.document_highlight =
                    DocumentHighlightHandler::new(sender(Feature::Highlights, &tx));
                handlers.document_links = DocumentLinksHandler::new(sender(Feature::Links, &tx));
                handlers.document_symbols =
                    DocumentSymbolsHandler::new(sender(Feature::Symbols, &tx));
            })
            .build()?;
        app.editor.open(path, Action::Replace)?;
        Ok(Self {
            app,
            gates,
            callbacks,
        })
    }

    async fn initialize(&mut self) -> anyhow::Result<Vec<EditorCallback>> {
        let mut responses = Vec::new();
        for index in 0..self.gates.len() {
            // Opening a document and each server initialization can restart feature
            // requests. Release one server only after the preceding batch has arrived,
            // so no startup response can leak into a later edit's batch.
            self.gates[index].release()?;
            let (server_id, call) = tokio::time::timeout(
                Duration::from_secs(10),
                self.app.editor.next_language_server_message(),
            )
            .await?
            .context("LSP message stream closed")?;
            anyhow::ensure!(
                matches!(&call, lsp_client::Call::Notification(message)
                if message.method == "initialized"),
                "expected initialization notification"
            );
            self.app
                .handle_language_server_message(call, server_id)
                .await;
            // Earlier batches are superseded by the next initialization. Leave the
            // final batch unpublished so tests can exercise stale-result rejection.
            responses = self.batch(ALL).await?;
        }
        Ok(responses)
    }

    async fn batch(&mut self, expected: &[Feature]) -> anyhow::Result<Vec<EditorCallback>> {
        let mut kinds = Vec::new();
        let mut callbacks = Vec::new();
        while kinds.len() < expected.len() {
            let (kind, scheduling, callback) =
                tokio::time::timeout(Duration::from_secs(10), self.callbacks.recv())
                    .await
                    .with_context(|| format!("expected {expected:?}, received {kinds:?}"))?
                    .expect("feature queue closed");
            // A selection change may queue a highlight request during startup.
            // Its position relative to other features' responses is nondeterministic.
            // Execute scheduling callbacks; only return actual responses to the test.
            if scheduling {
                callback(&mut self.app.editor);
                continue;
            }
            kinds.push(kind);
            callbacks.push(callback);
        }
        kinds.sort();
        assert_eq!(kinds, expected);
        Ok(callbacks)
    }

    fn apply(&mut self, callbacks: Vec<EditorCallback>) {
        for callback in callbacks {
            callback(&mut self.app.editor);
        }
    }

    fn show_colors(&mut self, enable: bool) {
        let mut config = (*self.app.editor.config()).clone();
        config.lsp.color_swatches.enable = enable;
        self.app
            .handle_config_events(ConfigEvent::Update(Box::new(config)));
    }

    fn edit(&mut self, word: &str) {
        let (view, doc) = current!(self.app.editor);
        let change = Transaction::change(
            doc.text(),
            [(2, doc.text().len_chars() - 1, Some(word.into()))].into_iter(),
        );
        view::editing::apply(doc, &change, view.id).unwrap();
    }

    fn assert_features(&self, text: &str) {
        let (view, doc) = current_ref!(self.app.editor);
        assert_eq!(
            doc.breadcrumbs(view.id)
                .unwrap()
                .iter()
                .map(|c| c.name.as_ref())
                .collect::<Vec<_>>(),
            [text]
        );
        let highlights = doc.document_highlights(view.id).unwrap();
        assert_eq!(highlights.len(), 1);
        assert_eq!(highlights[0], 2..text.chars().count());
        assert_eq!(doc.document_links().len(), 1);
        assert_eq!(
            (doc.document_links()[0].start, doc.document_links()[0].end),
            (2, text.chars().count())
        );
        assert_eq!(doc.color_swatches().unwrap().color_swatches[0].char_idx, 2);
        assert_eq!(doc.code_lenses().len(), 1);
        assert_eq!(doc.code_lenses()[0].char_idx, 2);
    }

    fn assert_empty(&self) {
        let (view, doc) = current_ref!(self.app.editor);
        assert!(doc
            .breadcrumbs(view.id)
            .is_none_or(|b| b.iter().next().is_none()));
        assert!(doc.document_highlights(view.id).is_none());
        assert!(doc.document_links().is_empty());
        assert!(doc.color_swatches().is_none());
        assert!(doc.code_lenses().is_empty());
    }
}

const ALL: &[Feature] = &[
    Feature::Lenses,
    Feature::Colors,
    Feature::Highlights,
    Feature::Links,
    Feature::Symbols,
];

#[tokio::test(flavor = "multi_thread")]
async fn color_configuration_refreshes_open_documents_and_rejects_queued_results(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut fixture = Fixture::new(&dir.path().join("colors.feature-test"), "😀 original\n")?;
    let initial = fixture.initialize().await?;
    fixture.show_colors(false);
    fixture.apply(initial);
    assert!(current_ref!(fixture.app.editor)
        .1
        .color_swatches()
        .is_none());
    let disabled = fixture.batch(&[Feature::Lenses]).await?;
    fixture.apply(disabled);

    // Enabling a feature on an already open document requests its data without an edit.
    fixture.show_colors(true);
    let enabled = fixture.batch(&[Feature::Lenses, Feature::Colors]).await?;
    fixture.apply(enabled);
    assert!(current_ref!(fixture.app.editor)
        .1
        .color_swatches()
        .is_some());

    // Cached swatches disappear immediately, and a response queued before disable
    // must not repopulate them even if the setting is subsequently enabled again.
    fixture.edit("replacement");
    let pending = fixture.batch(ALL).await?;
    fixture.show_colors(false);
    assert!(current_ref!(fixture.app.editor)
        .1
        .color_swatches()
        .is_none());
    let disabled = fixture.batch(&[Feature::Lenses]).await?;
    fixture.apply(disabled);
    fixture.show_colors(true);
    fixture.apply(pending);
    assert!(current_ref!(fixture.app.editor)
        .1
        .color_swatches()
        .is_none());
    let enabled = fixture.batch(&[Feature::Lenses, Feature::Colors]).await?;
    fixture.apply(enabled);
    assert!(current_ref!(fixture.app.editor)
        .1
        .color_swatches()
        .is_some());
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_events_and_results_stay_with_each_editor() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut first = Fixture::new(&dir.path().join("first.feature-test"), "😀 first\n")?;
    let mut second = Fixture::new(&dir.path().join("second.feature-test"), "😀 second\n")?;
    assert_eq!(
        current_ref!(first.app.editor).1.id(),
        current_ref!(second.app.editor).1.id()
    );
    for fixture in [&mut first, &mut second] {
        let callbacks = fixture.initialize().await?;
        fixture.apply(callbacks);
    }
    first.assert_features("😀 first");
    second.assert_features("😀 second");

    first.edit("updated");
    second.edit("another");
    for fixture in [&mut first, &mut second] {
        // Scheduling and publication may interleave across features.
        let responses = fixture.batch(ALL).await?;
        fixture.apply(responses);
    }
    first.assert_features("😀 updated");
    second.assert_features("😀 another");
    assert!(first.app.close().await.is_empty());
    assert!(second.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn language_changes_reject_queued_results_and_refresh_reused_servers() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut fixture = Fixture::new(&dir.path().join("language.feature-test"), "😀 original\n")?;
    let old = fixture.initialize().await?;
    let doc = current_ref!(fixture.app.editor).1;
    let id = doc.id();
    let server_id = doc.language_servers().next().unwrap().id();

    fixture.app.editor.set_document_language(id, None)?;
    fixture.assert_empty();
    fixture
        .app
        .editor
        .set_document_language(id, Some("document-feature-test"))?;
    assert_eq!(
        current_ref!(fixture.app.editor)
            .1
            .language_servers()
            .next()
            .unwrap()
            .id(),
        server_id
    );
    fixture.apply(old);
    fixture.assert_empty();
    let refreshed = fixture.batch(ALL).await?;
    fixture.apply(refreshed);
    fixture.assert_features("😀 original");
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_results_are_discarded_after_edits_and_document_closure() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut fixture = Fixture::new(&dir.path().join("edit.feature-test"), "😀 original\n")?;
    let old = fixture.initialize().await?;
    fixture.edit("replacement");
    fixture.apply(old);
    fixture.assert_empty();
    let fresh = fixture.batch(ALL).await?;
    fixture.apply(fresh);
    fixture.assert_features("😀 replacement");

    fixture.edit("closed");
    let responses = fixture.batch(ALL).await?;
    let id = current_ref!(fixture.app.editor).1.id();
    assert!(fixture.app.editor.close_document(id, true).is_ok());
    fixture.apply(responses);
    assert!(fixture.app.editor.document(id).is_none());
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_highlights_follow_the_selection_and_view_document() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut fixture = Fixture::new(&dir.path().join("selection.feature-test"), "😀 original\n")?;
    let old = fixture.initialize().await?;
    let (view, doc) = current!(fixture.app.editor);
    doc.set_selection(view.id, Selection::point(3));
    fixture.apply(old);
    let (view, doc) = current_ref!(fixture.app.editor);
    assert!(doc.document_highlights(view.id).is_none());
    let responses = fixture.batch(&[Feature::Highlights]).await?;
    let old_id = current_ref!(fixture.app.editor).1.id();
    fixture.app.editor.new_file(Action::Replace);
    fixture.apply(responses);
    let view = fixture.app.editor.tree.focus();
    assert!(fixture
        .app
        .editor
        .document(old_id)
        .unwrap()
        .document_highlights(view)
        .is_none());
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_results_from_detached_servers_are_discarded() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut fixture = Fixture::new(&dir.path().join("server.feature-test"), "😀 original\n")?;
    let responses = fixture.initialize().await?;
    let (_, doc) = current!(fixture.app.editor);
    doc.remove_language_server_by_name("document-feature-test")
        .unwrap();
    fixture.apply(responses);
    fixture.assert_empty();
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn remaining_servers_refresh_document_features_after_an_exit() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut fixture = Fixture::with_servers(
        &dir.path().join("exit.feature-test"),
        "😀 original\n",
        &["document-feature-test", "document-feature-other"],
    )?;
    let initial = fixture.initialize().await?;
    fixture.apply(initial);
    assert_eq!(current_ref!(fixture.app.editor).1.document_links().len(), 2);
    assert_eq!(current_ref!(fixture.app.editor).1.code_lenses().len(), 2);
    fixture.edit("updated");
    let old = fixture.batch(ALL).await?;
    let server_id = current_ref!(fixture.app.editor)
        .1
        .language_servers()
        .find(|server| server.name() == "document-feature-test")
        .unwrap()
        .id();
    fixture.app.editor.handle_language_server_exit(server_id);
    fixture.apply(old);
    let responses = fixture
        .batch(&[Feature::Lenses, Feature::Colors, Feature::Links])
        .await?;
    fixture.apply(responses);
    let (_, doc) = current_ref!(fixture.app.editor);
    assert_eq!(doc.document_links().len(), 1);
    assert_eq!(doc.color_swatches().unwrap().color_swatches.len(), 1);
    assert_eq!(doc.code_lenses().len(), 1);
    assert_ne!(doc.code_lenses()[0].language_server_id, server_id);
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

async fn prepare_link_follow(
    fixture: &mut Fixture,
) -> anyhow::Result<view::features::document_links::FileNavigationTask> {
    use view::features::document_links::{prepare_navigation, FileNavigation};
    let replies = fixture.initialize().await?;
    fixture.apply(replies);
    let (view, doc) = current!(fixture.app.editor);
    doc.set_selection(view.id, Selection::single(2, 3));
    match prepare_navigation(&mut fixture.app.editor) {
        FileNavigation::Pending(task) => Ok(task),
        FileNavigation::Ready(_) => anyhow::bail!("expected a deferred document link"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn resolved_link_navigation_validates_queued_delivery() -> anyhow::Result<()> {
    use view::features::document_links::NavigationTarget;
    for invalidation in [
        "none",
        "edit",
        "selection",
        "rebind",
        "focus",
        "close",
        "rename",
        "detach",
        "disabled",
        "superseded",
        "mode",
    ] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("document.feature-test");
        let mut fixture =
            Fixture::with_options(&path, "😀 original\n", &["document-feature-test"], true)?;
        let gate = Gate::new(path.with_extension("resolve-ready"));
        let task = prepare_link_follow(&mut fixture).await?;
        gate.release()?;
        let update = tokio::time::timeout(Duration::from_secs(5), task.run()).await?;
        let id = current_ref!(fixture.app.editor).1.id();
        let expected = lsp_client::Url::parse(&format!(
            "{}.resolved",
            current_ref!(fixture.app.editor).1.url().unwrap()
        ))?;
        match invalidation {
            "edit" => fixture.edit("changed"),
            "selection" => {
                let (view, doc) = current!(fixture.app.editor);
                doc.set_selection(view.id, Selection::single(3, 4));
                doc.set_selection(view.id, Selection::single(2, 3));
            }
            "rebind" => {
                fixture.app.editor.new_file(Action::Replace);
                fixture.app.editor.switch(id, Action::Replace);
            }
            "focus" => {
                fixture.app.editor.new_file(Action::VerticalSplit);
            }
            "close" => {
                assert!(fixture.app.editor.close_document(id, true).is_ok());
            }
            "rename" => fixture
                .app
                .editor
                .set_doc_path(id, &dir.path().join("renamed.feature-test")),
            "detach" => {
                fixture
                    .app
                    .editor
                    .document_mut(id)
                    .unwrap()
                    .remove_language_server_by_name("document-feature-test");
            }
            "superseded" => {
                let _ = view::features::document_links::prepare_navigation(&mut fixture.app.editor);
            }
            "mode" => fixture.app.editor.set_mode(view::document::Mode::Insert),
            "disabled" => {
                let mut config = (*fixture.app.editor.config()).clone();
                config.lsp.enable = false;
                fixture.app.editor.config =
                    std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(config));
            }
            _ => {}
        }
        let targets = update.into_targets(&fixture.app.editor);
        if invalidation == "none" {
            assert_eq!(targets, Some(vec![NavigationTarget::Url(expected)]));
        } else {
            assert!(
                targets.is_none(),
                "stale navigation survived {invalidation}"
            );
        }
        assert!(fixture.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn origin_changes_cancel_pending_link_resolution() -> anyhow::Result<()> {
    for invalidation in ["selection", "view", "path", "language"] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("document.feature-test");
        let mut fixture =
            Fixture::with_options(&path, "😀 original\n", &["document-feature-test"], true)?;
        let gate = Gate::new(path.with_extension("resolve-ready"));
        let task = prepare_link_follow(&mut fixture).await?;
        let task = tokio::spawn(task.run());
        tokio::time::timeout(Duration::from_secs(5), async {
            while !gate.path().with_extension("started").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        let (view, doc) = current_ref!(fixture.app.editor);
        let (view, id) = (view.id, doc.id());
        match invalidation {
            "selection" => fixture
                .app
                .editor
                .document_mut(id)
                .unwrap()
                .set_selection(view, Selection::single(3, 4)),
            "view" => fixture.app.editor.close(view),
            "path" => fixture
                .app
                .editor
                .set_doc_path(id, &dir.path().join("renamed.feature-test")),
            _ => {
                fixture.app.editor.set_document_language(id, None)?;
            }
        }
        // Cancellation completes locally before the server releases its held reply.
        let update = tokio::time::timeout(Duration::from_secs(5), task).await??;
        assert!(update.into_targets(&fixture.app.editor).is_none());
        gate.release()?;
        assert!(fixture.app.close().await.is_empty());
    }
    Ok(())
}

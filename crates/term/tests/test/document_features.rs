use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use editor_core::{Selection, Transaction};
use term::application::Application;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use view::{
    callbacks::{EditorCallback, EditorCallbackSender},
    current, current_ref,
    editor::Action,
    handlers::{
        document_colors::DocumentColorsHandler, document_highlight::DocumentHighlightHandler,
        document_links::DocumentLinksHandler, document_symbols::DocumentSymbolsHandler,
    },
};

use super::helpers::{test_config, test_syntax_loader, AppBuilder};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Feature {
    Colors,
    Highlights,
    Links,
    Symbols,
}

struct Fixture {
    app: Application,
    gates: Vec<PathBuf>,
    callbacks: mpsc::Receiver<(Feature, bool, EditorCallback)>,
}

fn sender(
    feature: Feature,
    tx: &mpsc::Sender<(Feature, bool, EditorCallback)>,
) -> EditorCallbackSender {
    let tx = tx.clone();
    let blocking = tx.clone();
    EditorCallbackSender::new(
        move |callback| {
            let tx = tx.clone();
            async move {
                let _ = tx.send((feature, false, callback)).await;
            }
        },
        move |callback| event::send_blocking(&blocking, (feature, true, callback)),
    )
}

impl Fixture {
    fn new(path: &Path, text: &str) -> anyhow::Result<Self> {
        Self::with_servers(path, text, &["document-feature-test"])
    }

    fn with_servers(path: &Path, text: &str, servers: &[&str]) -> anyhow::Result<Self> {
        std::fs::write(path, text)?;
        let mut config = test_config();
        config.editor.lsp.enable = true;
        config.editor.lsp.auto_document_highlight = true;
        config.editor.breadcrumb.enable = true;
        let command = toml::Value::String(env!("CARGO_BIN_EXE_mitos-test-lsp").into());
        let gates: Vec<_> = servers
            .iter()
            .map(|name| path.with_extension(format!("{name}.initialize-ready")))
            .collect();
        let server_config = servers
            .iter()
            .zip(&gates)
            .map(|(name, gate)| {
                let args = toml::Value::Array(
                    ["--initialize-gate", gate.to_str().unwrap()]
                        .into_iter()
                        .map(|arg| toml::Value::String(arg.into()))
                        .collect(),
                );
                format!("[language-server.{name}]\ncommand = {command}\nargs = {args}\n")
            })
            .collect::<String>();
        let servers = toml::Value::Array(
            servers
                .iter()
                .map(|name| toml::Value::String((*name).into()))
                .collect(),
        );
        let loader = test_syntax_loader(Some(format!(
            r#"
            {server_config}
            [[language]]
            name = "document-feature-test"
            scope = "source.document-feature-test"
            file-types = ["feature-test"]
            roots = []
            language-servers = {servers}
        "#
        )));
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(loader)
            .build()?;
        let (tx, callbacks) = mpsc::channel(64);
        // Use independent queues so publication can be held while the editor changes.
        app.editor.handlers.document_colors =
            DocumentColorsHandler::new(sender(Feature::Colors, &tx));
        app.editor.handlers.document_highlight =
            DocumentHighlightHandler::new(sender(Feature::Highlights, &tx));
        app.editor.handlers.document_links = DocumentLinksHandler::new(sender(Feature::Links, &tx));
        app.editor.handlers.document_symbols =
            DocumentSymbolsHandler::new(sender(Feature::Symbols, &tx));
        app.editor.open(path, Action::Replace)?;
        Ok(Self {
            app,
            gates,
            callbacks,
        })
    }

    async fn initialize(&mut self) -> anyhow::Result<Vec<EditorCallback>> {
        let mut responses = Vec::new();
        for gate in self.gates.clone() {
            // Opening a document and each server initialization can restart feature
            // requests. Release one server only after the preceding batch has arrived,
            // so no startup response can leak into a later edit's batch.
            std::fs::write(gate, "ready")?;
            let (server_id, call) = tokio::time::timeout(
                Duration::from_secs(10),
                self.app.editor.language_servers.incoming.next(),
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

    fn edit(&mut self, word: &str) {
        let (view, doc) = current!(self.app.editor);
        let change = Transaction::change(
            doc.text(),
            [(2, doc.text().len_chars() - 1, Some(word.into()))].into_iter(),
        );
        assert!(doc.apply(&change, view.id));
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
        assert_eq!(doc.document_links.len(), 1);
        assert_eq!(
            (doc.document_links[0].start, doc.document_links[0].end),
            (2, text.chars().count())
        );
        assert_eq!(
            doc.color_swatches.as_ref().unwrap().color_swatches[0].char_idx,
            2
        );
    }

    fn assert_empty(&self) {
        let (view, doc) = current_ref!(self.app.editor);
        assert!(doc
            .breadcrumbs(view.id)
            .is_none_or(|b| b.iter().next().is_none()));
        assert!(doc.document_highlights(view.id).is_none());
        assert!(doc.document_links.is_empty());
        assert!(doc.color_swatches.is_none());
    }
}

const ALL: &[Feature] = &[
    Feature::Colors,
    Feature::Highlights,
    Feature::Links,
    Feature::Symbols,
];

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
    let view = fixture.app.editor.tree.focus;
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
async fn remaining_servers_refresh_colors_and_links_after_an_exit() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut fixture = Fixture::with_servers(
        &dir.path().join("exit.feature-test"),
        "😀 original\n",
        &["document-feature-test", "document-feature-other"],
    )?;
    let initial = fixture.initialize().await?;
    fixture.apply(initial);
    assert_eq!(current_ref!(fixture.app.editor).1.document_links.len(), 2);
    fixture.edit("updated");
    let old = fixture.batch(ALL).await?;
    let server_id = current_ref!(fixture.app.editor)
        .1
        .language_servers()
        .find(|server| server.name() == "document-feature-test")
        .unwrap()
        .id();
    event::dispatch(view::events::LanguageServerExited {
        editor: &mut fixture.app.editor,
        server_id,
    });
    fixture.app.editor.language_servers.remove_by_id(server_id);
    fixture.apply(old);
    let responses = fixture.batch(&[Feature::Colors, Feature::Links]).await?;
    fixture.apply(responses);
    let (_, doc) = current_ref!(fixture.app.editor);
    assert_eq!(doc.document_links.len(), 1);
    assert_eq!(doc.color_swatches.as_ref().unwrap().color_swatches.len(), 1);
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

use std::{path::PathBuf, time::Duration};

use anyhow::Context as _;
use serde_json::Value;
use term::application::Application;
use tokio::sync::mpsc;
use view::{callbacks::EditorCallback, editor::Action, DocumentId};

use super::super::helpers::{
    callbacks,
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};
use super::{wait_for_request, FileOperation};

pub(super) struct Fixture {
    pub(super) app: Application,
    callbacks: mpsc::UnboundedReceiver<EditorCallback>,
    pub(super) dir: tempfile::TempDir,
    gate: Gate,
    pub(super) source: PathBuf,
    pub(super) target: PathBuf,
    pub(super) dependent: PathBuf,
}

impl Fixture {
    pub(super) async fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let initialize_gate = Gate::new(dir.path().join("initialize"));
        let gate = Gate::new(dir.path().join("preflight"));
        let server = ServerConfig::feature("scope", "--file-operations", dir.path())
            .initialize_gate(&initialize_gate)
            .arg("--preflight-gate")
            .arg(gate.path())
            .arg("--preflight-edit")
            .arg(dir.path().join("edit.json"))
            .toml();
        let mut config = test_config();
        config.editor.lsp.enable = true;
        let (sender, callbacks) = callbacks::deferred(|_, callback| callback);
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(lsp::syntax_loader("scope", "scope", &["scope"], &server))
            .with_handler_setup(move |handlers, config| {
                *handlers =
                    view::handlers::Handlers::new(config, sender(handlers.redraw().clone()));
            })
            .build()?;
        let source = dir.path().join("source.scope");
        let target = dir.path().join("target.scope");
        let dependent = dir.path().join("dependent.txt");
        std::fs::write(&source, "source\n")?;
        std::fs::write(&dependent, "dependent\n")?;
        app.editor.open(&source, Action::Replace)?;
        initialize_gate.release()?;
        lsp::initialize(&mut app, 1).await?;
        Ok(Self {
            app,
            callbacks,
            dir,
            gate,
            source,
            target,
            dependent,
        })
    }

    pub(super) async fn request_rename(&mut self, edit: Value) -> anyhow::Result<()> {
        std::fs::write(
            self.dir.path().join("edit.json"),
            serde_json::to_vec(&edit)?,
        )?;
        self.app
            .editor
            .request_file_operation(FileOperation::Move {
                from: self.source.clone(),
                to: self.target.clone(),
            })?;
        wait_for_request(&[lsp::log_path(self.dir.path(), "scope")]).await
    }

    pub(super) async fn flush_notifications(&self) -> anyhow::Result<Vec<Value>> {
        // A response is a FIFO barrier for earlier notifications on this transport.
        std::fs::write(self.dir.path().join("edit.json"), "null")?;
        self.gate.release()?;
        let server = self
            .app
            .editor
            .language_servers
            .iter_clients()
            .next()
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            server
                .will_create(&self.dir.path().join("barrier"), false)
                .unwrap(),
        )
        .await??;
        lsp::read_log(&lsp::log_path(self.dir.path(), "scope"))
    }

    pub(super) async fn complete_rename(&mut self, succeeds: bool) -> anyhow::Result<()> {
        self.gate.release()?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let callback = self.callbacks.recv().await.context("missing callback")?;
                callback(&mut self.app.editor);
                if !self.source.exists()
                    || self
                        .app
                        .editor
                        .status_msg
                        .as_ref()
                        .is_some_and(|(message, _)| {
                            message.contains("Could not complete file operation")
                        })
                {
                    break;
                }
            }
            anyhow::Ok(())
        })
        .await??;
        assert_eq!(self.source.exists(), !succeeds);
        assert_eq!(self.target.exists(), succeeds);
        if !succeeds {
            assert!(self
                .app
                .editor
                .status_msg
                .as_ref()
                .is_some_and(|(message, _)| { message.contains("Documents changed") }));
        }
        Ok(())
    }

    pub(super) fn change(&mut self, id: DocumentId, change: &str) -> anyhow::Result<()> {
        match change {
            "edit" => {
                let doc = self.app.editor.document(id).unwrap();
                let transaction = editor_core::Transaction::change(
                    doc.text(),
                    [(0, 0, Some("typed ".into()))].into_iter(),
                );
                self.app
                    .editor
                    .apply_document_changes(id, doc.version(), transaction.changes())?;
            }
            "close" => assert!(self.app.editor.close_document(id, true).is_ok()),
            "rename" => self
                .app
                .editor
                .set_doc_path(id, &self.dir.path().join("other.txt")),
            "reopen" => {
                assert!(self.app.editor.close_document(id, true).is_ok());
                self.app.editor.open(&self.dependent, Action::Load)?;
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}

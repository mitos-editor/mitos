//! Exercise asynchronous action resolution through the public editor callback API.
use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::Context as _;
use arc_swap::ArcSwap;
use editor_core::Transaction;
use lsp_client::{lsp, LanguageServerId};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use view::{
    action::{Action, LspActionContext},
    callbacks::EditorCallback,
    config::Config,
    current, current_ref,
    editor::Action as OpenAction,
    features::{document_links::DocumentLinksHandler, syntax::SyntaxHandler},
    graphics::Rect,
    handlers::Handlers,
    theme, DocumentId, Editor,
};

use super::helpers::{
    callbacks,
    lsp::{self as fixture_lsp, Gate, ServerConfig},
};

struct Fixture {
    editor: Editor,
    callbacks: mpsc::UnboundedReceiver<(bool, EditorCallback)>,
    doc: DocumentId,
    server: LanguageServerId,
    client: Arc<lsp_client::Client>,
    log: PathBuf,
    gate: Gate,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> anyhow::Result<Self> {
        Self::with_resolve(true).await
    }

    async fn with_resolve(resolve: bool) -> anyhow::Result<Self> {
        Self::with_save_actions(resolve, &[], false).await
    }

    async fn with_save_actions(
        resolve: bool,
        kinds: &[&str],
        format: bool,
    ) -> anyhow::Result<Self> {
        let save = !kinds.is_empty();
        let dir = tempfile::tempdir()?;
        let log = fixture_lsp::log_path(dir.path(), "alpha");
        let gate = Gate::new(dir.path().join("resolve-ready"));
        let initialize = Gate::new(dir.path().join("initialize-ready"));
        let server = ServerConfig::feature("alpha", "--action-execution", dir.path())
            .initialize_gate(&initialize)
            .arg("--response-gate")
            .arg(gate.path())
            .arg(if resolve { "--resolve" } else { "--no-resolve" })
            .arg(if save {
                "--save-actions"
            } else {
                "--interactive-actions"
            })
            .toml();
        let kinds = serde_json::to_string(kinds)?;
        let formatter = if format {
            r#"formatter = { command = "bash", args = ["-c", "tr '[:lower:]' '[:upper:]'"] }"#
        } else {
            ""
        };
        let languages = super::helpers::test_syntax_loader(Some(format!(
            r#"
            {server}
            [[language]]
            name = "action-test"
            scope = "source.action-test"
            file-types = ["action-test"]
            language-servers = ["alpha"]
            roots = []
            code-actions-on-save = {kinds}
            auto-format = {format}
            {formatter}
        "#
        )));
        let mut config = Config::default();
        config.files.watch.enable = false;
        config.files.reload.enable = false;
        config.completion.auto_trigger = false;
        config.lsp.color_swatches.enable = false;
        if save {
            config.format.on_save = true;
        }
        let (sender, callbacks) = callbacks::unbounded(|blocking, callback| (blocking, callback));
        let mut handlers = Handlers::new(&config, sender);
        // Links and syntax publish empty async results even without a provider.
        // Keep their destination separate so this queue contains only action replies.
        let (background, _background_callbacks) = callbacks::channel();
        handlers.document_links = DocumentLinksHandler::new(background.clone());
        handlers.syntax = SyntaxHandler::new(background);
        let mut editor = Editor::new(
            Rect::new(0, 0, 80, 24),
            Arc::new(theme::Loader::new(loader::theme::Resources::new(vec![]))),
            Arc::new(ArcSwap::from_pointee(languages)),
            Arc::new(ArcSwap::from_pointee(config)),
            handlers,
            workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
        );
        editor.new_file(OpenAction::VerticalSplit);
        let path = dir.path().join("document.action-test");
        std::fs::write(&path, "old\n")?;
        editor.open(&path, OpenAction::Replace)?;
        initialize.release()?;
        let (server, call) = tokio::time::timeout(
            Duration::from_secs(10),
            editor.language_servers.incoming.next(),
        )
        .await?
        .context("missing initialization")?;
        anyhow::ensure!(
            matches!(call, lsp_client::Call::Notification(ref call) if call.method == "initialized")
        );
        editor.handle_language_server_initialized(server);
        let doc = current_ref!(editor).1.id();
        let client = editor.language_servers.get_by_id(server).unwrap().clone();
        Ok(Self {
            editor,
            callbacks,
            doc,
            server,
            client,
            log,
            gate,
            _dir: dir,
        })
    }

    fn action(&self, failure: &str) -> Action {
        let doc = self.editor.document(self.doc).unwrap();
        Action::lsp(
            LspActionContext::new(doc),
            self.server,
            lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
                title: "fixture action".into(),
                data: Some(
                    json!({"uri": doc.url().unwrap(), "version": doc.version(), "failure": failure}),
                ),
                ..Default::default()
            }),
        )
    }

    fn messages(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    async fn wait_request(&self, method: &str) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !self
                .messages()
                .iter()
                .any(|message| message["method"] == method)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        Ok(())
    }

    async fn resolved(&mut self) -> anyhow::Result<EditorCallback> {
        self.wait_request("codeAction/resolve").await?;
        self.gate.release()?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (blocking, callback) = self
                    .callbacks
                    .recv()
                    .await
                    .context("missing resolution callback")?;
                if !blocking {
                    return Ok(callback);
                }
                // Opening/focusing documents queues synchronous feature invalidations.
                // Other asynchronous feature publishers are isolated above, so only
                // code-action resolution publishes through the async delivery path.
                callback(&mut self.editor);
            }
        })
        .await?
    }

    async fn assert_no_command(&self) -> anyhow::Result<()> {
        // A transport barrier proves earlier requests reached the server, avoiding
        // an absence assertion that merely races the process reading its input.
        self.client
            .notify::<lsp::notification::DidChangeConfiguration>(
                lsp::DidChangeConfigurationParams {
                    settings: json!({"barrier": true}),
                },
            );
        self.wait_request("workspace/didChangeConfiguration")
            .await?;
        assert!(!self
            .messages()
            .iter()
            .any(|message| message["method"] == "workspace/executeCommand"));
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn resolution_yields_and_applies_to_origin_before_command() -> anyhow::Result<()> {
    let mut f = Fixture::new().await?;
    f.action("").execute(&mut f.editor);
    // Resolve cannot finish until the gate opens. Returning here proves it did not block.
    f.wait_request("codeAction/resolve").await?;
    assert!(!f.gate.path().exists());
    f.editor.new_file(OpenAction::Replace);
    let other = current_ref!(f.editor).1.id();
    let other_text = current_ref!(f.editor).1.text().to_string();
    let callback = f.resolved().await?;
    callback(&mut f.editor);
    f.wait_request("workspace/executeCommand").await?;
    assert_eq!(
        f.editor.document(f.doc).unwrap().text().to_string(),
        "fixed\n"
    );
    assert_eq!(
        f.editor.document(other).unwrap().text().to_string(),
        other_text
    );
    let messages = f.messages();
    let edit = messages
        .iter()
        .position(|m| m["method"] == "textDocument/didChange")
        .unwrap();
    let command = messages
        .iter()
        .position(|m| m["method"] == "workspace/executeCommand")
        .unwrap();
    assert!(edit < command);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_resolution_rejects_changed_document() -> anyhow::Result<()> {
    let mut f = Fixture::new().await?;
    f.action("").execute(&mut f.editor);
    let callback = f.resolved().await?;
    let (view, doc) = current!(f.editor);
    view::editing::apply(
        doc,
        &Transaction::insert(
            doc.text(),
            &doc.selection(view.id).clone(),
            "changed".into(),
        ),
        view.id,
    );
    callback(&mut f.editor);
    assert!(f
        .editor
        .document(f.doc)
        .unwrap()
        .text()
        .to_string()
        .contains("changed"));
    assert!(f
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("no longer current"));
    f.assert_no_command().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn resolution_and_edit_failures_are_reported_without_commands() -> anyhow::Result<()> {
    for failure in ["resolve", "edit"] {
        let mut f = Fixture::new().await?;
        f.action(failure).execute(&mut f.editor);
        let callback = f.resolved().await?;
        callback(&mut f.editor);
        assert_eq!(
            f.editor.document(f.doc).unwrap().text().to_string(),
            "old\n"
        );
        assert!(f
            .editor
            .get_status()
            .unwrap()
            .0
            .contains(if failure == "resolve" {
                "Failed to resolve code action"
            } else {
                "Failed to apply code action"
            }));
        f.assert_no_command().await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn actions_capture_request_snapshot_before_selection() -> anyhow::Result<()> {
    let mut f = Fixture::new().await?;
    let action = f.action("");
    let (view, doc) = current!(f.editor);
    view::editing::apply(
        doc,
        &Transaction::insert(
            doc.text(),
            &doc.selection(view.id).clone(),
            "changed".into(),
        ),
        view.id,
    );
    action.execute(&mut f.editor);
    assert!(f
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("no longer current"));
    assert!(!f
        .messages()
        .iter()
        .any(|message| message["method"] == "codeAction/resolve"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_resolution_rejects_closed_or_detached_origins() -> anyhow::Result<()> {
    for invalidation in ["close", "detach", "server", "rename"] {
        let mut f = Fixture::new().await?;
        f.action("").execute(&mut f.editor);
        let callback = f.resolved().await?;
        match invalidation {
            "close" => assert!(f.editor.close_document(f.doc, true).is_ok()),
            "rename" => {
                f.editor
                    .set_doc_path(f.doc, &f._dir.path().join("renamed.action-test"));
            }
            "detach" => {
                f.editor
                    .document_mut(f.doc)
                    .unwrap()
                    .remove_language_server_by_name("alpha");
            }
            _ => f.editor.language_servers.remove_by_id(f.server),
        }
        callback(&mut f.editor);
        assert!(f
            .editor
            .get_status()
            .unwrap()
            .0
            .contains("no longer current"));
        f.assert_no_command().await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn action_and_queued_reply_stay_bound_to_their_editor() -> anyhow::Result<()> {
    let mut origin = Fixture::new().await?;
    let mut other = Fixture::new().await?;
    // Make all value-based origin checks collide across the two editors.
    let path = origin
        .editor
        .document(origin.doc)
        .unwrap()
        .path()
        .unwrap()
        .to_owned();
    other.editor.set_doc_path(other.doc, &path);
    assert_eq!(origin.doc, other.doc);
    assert_eq!(origin.server, other.server);
    let action = origin.action("");
    action.execute(&mut other.editor);
    assert!(other
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("no longer current"));
    action.execute(&mut origin.editor);
    let callback = origin.resolved().await?;
    callback(&mut other.editor);
    assert_eq!(
        other.editor.document(other.doc).unwrap().text().to_string(),
        "old\n"
    );
    assert!(other
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("no longer current"));
    other.assert_no_command().await?;
    origin.assert_no_command().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn actions_without_resolve_support_and_local_actions_stay_synchronous() -> anyhow::Result<()>
{
    let mut f = Fixture::with_resolve(false).await?;
    let doc = f.editor.document(f.doc).unwrap();
    let action = serde_json::from_value(json!({
        "title": "immediate edit",
        "edit": {"documentChanges": [{
            "textDocument": {"uri": doc.url().unwrap(), "version": doc.version()},
            "edits": [{"range": {"start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 3}}, "newText": "fixed"}]
        }]}
    }))?;
    Action::lsp(LspActionContext::new(doc), f.server, action).execute(&mut f.editor);
    assert_eq!(
        f.editor.document(f.doc).unwrap().text().to_string(),
        "fixed\n"
    );
    f.assert_no_command().await?;
    assert!(!f
        .messages()
        .iter()
        .any(|m| m["method"] == "codeAction/resolve"));
    Action::new("local", 0, |editor| editor.set_status("local action ran")).execute(&mut f.editor);
    assert_eq!(
        f.editor.get_status().unwrap().0.as_ref(),
        "local action ran"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn save_actions_run_in_order_before_formatting_the_latest_text() -> anyhow::Result<()> {
    use view::save::{self, SaveProgress, WriteOptions};

    let mut f = Fixture::with_save_actions(true, &["source.first", "source.second"], true).await?;
    let view = current_ref!(f.editor).0.id;
    let request = save::prepare(
        &mut f.editor,
        f.doc,
        Some(view),
        None,
        WriteOptions {
            force: false,
            auto_format: true,
            code_actions: true,
        },
    );
    f.gate.release()?;
    let mut progress = save::submit(&mut f.editor, request)?;
    while let SaveProgress::Pending(task) = progress {
        let update = tokio::time::timeout(Duration::from_secs(10), task.run()).await??;
        progress = update.apply(&mut f.editor)?;
    }
    assert!(matches!(progress, SaveProgress::Queued));
    f.editor.flush_writes().await?;
    let doc = f.editor.document(f.doc).unwrap();
    assert_eq!(doc.text().to_string(), "SECOND\n");
    assert_eq!(std::fs::read_to_string(doc.path().unwrap())?, "SECOND\n");
    let resolutions: Vec<_> = f
        .messages()
        .into_iter()
        .filter(|message| message["method"] == "codeAction/resolve")
        .collect();
    assert_eq!(resolutions.len(), 2);
    assert_eq!(resolutions[0]["params"]["data"]["end"], 3);
    assert_eq!(resolutions[1]["params"]["data"]["end"], 5);
    f.assert_no_command().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn save_actions_discard_stale_requests_and_resolutions_before_writing_current_text(
) -> anyhow::Result<()> {
    use view::save::{self, SaveProgress, WriteOptions};

    for during_resolve in [false, true] {
        let mut f = Fixture::with_save_actions(true, &["source.first"], false).await?;
        let view = current_ref!(f.editor).0.id;
        let request = save::prepare(
            &mut f.editor,
            f.doc,
            Some(view),
            None,
            WriteOptions {
                force: false,
                auto_format: false,
                code_actions: true,
            },
        );
        let SaveProgress::Pending(task) = save::submit(&mut f.editor, request)? else {
            anyhow::bail!("expected action request")
        };
        let mut update = tokio::time::timeout(Duration::from_secs(10), task.run()).await??;
        if during_resolve {
            let SaveProgress::Pending(task) = update.apply(&mut f.editor)? else {
                anyhow::bail!("expected action resolution")
            };
            f.gate.release()?;
            update = tokio::time::timeout(Duration::from_secs(10), task.run()).await??;
        }
        let (view, doc) = current!(f.editor);
        let edit = Transaction::change(doc.text(), [(0, 3, Some("newer".into()))].into_iter());
        view::editing::apply(doc, &edit, view.id);
        let mut progress = update.apply(&mut f.editor)?;
        while let SaveProgress::Pending(task) = progress {
            let update = tokio::time::timeout(Duration::from_secs(10), task.run()).await??;
            progress = update.apply(&mut f.editor)?;
        }
        assert!(matches!(progress, SaveProgress::Queued));
        f.editor.flush_writes().await?;
        let doc = f.editor.document(f.doc).unwrap();
        assert_eq!(doc.text().to_string(), "newer\n");
        assert_eq!(std::fs::read_to_string(doc.path().unwrap())?, "newer\n");
    }
    Ok(())
}

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use editor_core::Selection;
use term::application::Application;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use view::{
    callbacks::{EditorCallback, EditorCallbackSender},
    current, current_ref,
    document::Mode,
    editor::{Action, EditorEvent},
    handlers::completion::{
        self, CompletionChange, CompletionEvent, CompletionHandler, CompletionItem,
        CompletionUpdate,
    },
};

use super::helpers::{test_config, test_syntax_loader, AppBuilder};

struct Fixture {
    app: Application,
    callbacks: mpsc::UnboundedReceiver<(bool, EditorCallback)>,
    log: PathBuf,
    response_gate: PathBuf,
    initialize_gate: PathBuf,
    server_count: usize,
}

impl Fixture {
    async fn new(dir: &Path, text: &str, names: &[&str]) -> anyhow::Result<Self> {
        let mut fixture = Self::uninitialized(dir, text, names)?;
        fixture.initialize().await?;
        Ok(fixture)
    }

    fn uninitialized(dir: &Path, text: &str, names: &[&str]) -> anyhow::Result<Self> {
        let gate = dir.join("initialize-ready");
        if gate.exists() {
            std::fs::remove_file(&gate)?;
        }
        let response_gate = dir.join("alpha-ready");
        std::fs::write(&response_gate, "ready")?;
        let command = toml::Value::String(env!("CARGO_BIN_EXE_mitos-test-lsp").into());
        let mut servers = String::new();
        for name in names {
            let response_gate = dir.join(format!("{name}-ready"));
            std::fs::write(&response_gate, "ready")?;
            let log = dir.join(format!("{name}.jsonl"));
            let args = toml::Value::Array(
                [
                    "--completion",
                    name,
                    log.to_str().unwrap(),
                    "--initialize-gate",
                    gate.to_str().unwrap(),
                    "--response-gate",
                    response_gate.to_str().unwrap(),
                ]
                .into_iter()
                .map(|arg| toml::Value::String(arg.into()))
                .collect(),
            );
            servers.push_str(&format!(
                "[language-server.{name}]\ncommand = {command}\nargs = {args}\n"
            ));
        }
        let names_config = toml::Value::Array(
            names
                .iter()
                .map(|name| toml::Value::String((*name).into()))
                .collect(),
        );
        let loader = test_syntax_loader(Some(format!(
            r#"
            {servers}
            [[language]]
            name = "completion-test"
            scope = "source.completion-test"
            file-types = ["completion-test"]
            roots = []
            language-servers = {names_config}
        "#
        )));
        let mut config = test_config();
        config.editor.lsp.enable = true;
        config.editor.auto_completion = false;
        config.editor.path_completion = false;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(loader)
            .build()?;
        let (tx, callbacks) = mpsc::unbounded_channel();
        let blocking = tx.clone();
        app.editor.handlers.completions = CompletionHandler::new(
            EditorCallbackSender::new(
                move |callback| {
                    let tx = tx.clone();
                    async move {
                        let _ = tx.send((false, callback));
                    }
                },
                move |callback| {
                    let _ = blocking.send((true, callback));
                },
            ),
            &app.editor.config(),
        );
        let path = dir.join("document.completion-test");
        std::fs::write(&path, text)?;
        app.editor.open(&path, Action::Replace)?;
        let (view, doc) = current!(app.editor);
        doc.set_selection(view.id, Selection::point(doc.text().len_chars() - 1));
        app.editor.mode = Mode::Insert;
        while completion::next_update(&mut app.editor).is_some() {}
        Ok(Self {
            app,
            callbacks,
            log: dir.join("alpha.jsonl"),
            response_gate,
            initialize_gate: gate,
            server_count: names.len(),
        })
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        std::fs::write(&self.initialize_gate, "ready")?;
        for _ in 0..self.server_count {
            let (server, call) = tokio::time::timeout(
                Duration::from_secs(10),
                self.app.editor.language_servers.incoming.next(),
            )
            .await?
            .context("server initialization")?;
            anyhow::ensure!(
                matches!(&call, lsp_client::Call::Notification(message) if message.method == "initialized")
            );
            self.app.handle_language_server_message(call, server).await;
        }
        Ok(())
    }

    fn trigger(&self) {
        let (view, doc) = current_ref!(self.app.editor);
        self.app
            .editor
            .handlers
            .completions
            .event(CompletionEvent::ManualTrigger {
                cursor: doc
                    .selection(view.id)
                    .primary()
                    .cursor(doc.text().slice(..)),
                doc: doc.id(),
                view: view.id,
            });
    }

    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        loop {
            let (scheduling, callback) =
                tokio::time::timeout(Duration::from_secs(5), self.callbacks.recv())
                    .await?
                    .context("completion callback queue closed")?;
            if scheduling {
                callback(&mut self.app.editor);
                if let Some(update) = completion::next_update(&mut self.app.editor) {
                    self.app
                        .handle_editor_event(EditorEvent::Completion(update))
                        .await;
                }
            } else {
                return Ok(callback);
            }
        }
    }

    async fn update(&mut self) -> anyhow::Result<CompletionUpdate> {
        self.response().await?(&mut self.app.editor);
        completion::next_update(&mut self.app.editor).context("completion update missing")
    }

    async fn items(&mut self) -> anyhow::Result<Vec<CompletionItem>> {
        match self.update().await?.apply(&mut self.app.editor) {
            Some(CompletionChange::Show { items, .. }) => Ok(items),
            _ => anyhow::bail!("expected completion items"),
        }
    }

    async fn key(&mut self, key: &str) -> anyhow::Result<()> {
        #[cfg(not(windows))]
        let event = termina::event::Event::Key(ui_core::input::parse_macro(key)?[0].into());
        #[cfg(windows)]
        let event = crossterm::event::Event::Key(ui_core::input::parse_macro(key)?[0].into());
        self.app.handle_terminal_events(Ok(event)).await;
        Ok(())
    }

    fn text(&self) -> String {
        current_ref!(self.app.editor).1.text().to_string()
    }

    async fn show(&mut self) -> anyhow::Result<()> {
        self.trigger();
        let update = self.update().await?;
        self.app
            .handle_editor_event(EditorEvent::Completion(update))
            .await;
        assert!(self.app.editor.last_completion.is_some());
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_providers_preserve_protocol_ordering_and_incomplete_refresh() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 incomplete ap\n", &["alpha", "beta"]).await?;
    f.trigger();
    let items = f.items().await?;
    assert_eq!(items.len(), 4);
    for (priority, name) in [(0, "alpha"), (-1, "beta")] {
        let items: Vec<_> = items
            .iter()
            .filter(|item| item.provider_priority() == priority)
            .collect();
        assert_eq!(items[0].filter_text(), "apricot");
        assert_eq!(items[1].filter_text(), "apple");
        let requests = std::fs::read_to_string(dir.path().join(format!("{name}.jsonl")))?;
        let request: serde_json::Value = serde_json::from_str(requests.lines().last().unwrap())?;
        assert_eq!(request["position"]["character"], 16);
        assert_eq!(request["context"]["triggerKind"], 1);
    }
    f.app.editor.last_completion = Some(view::editor::CompleteAction::Triggered);
    completion::request_incomplete_completion_list(&mut f.app.editor);
    for _ in 0..2 {
        assert!(matches!(
            f.update().await?.apply(&mut f.app.editor),
            Some(CompletionChange::Provider {
                is_incomplete: true,
                ..
            })
        ));
    }
    let requests = std::fs::read_to_string(&f.log)?;
    let request: serde_json::Value = serde_json::from_str(requests.lines().last().unwrap())?;
    assert_eq!(request["context"]["triggerKind"], 3);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_survives_but_cancel_mode_rename_and_focus_reject_old_results() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 ap\n", &["alpha"]).await?;
    f.trigger();
    let old = f.response().await?;
    f.key("p").await?;
    old(&mut f.app.editor);
    let update = completion::next_update(&mut f.app.editor).unwrap();
    assert!(matches!(
        update.apply(&mut f.app.editor),
        Some(CompletionChange::Show { .. })
    ));
    f.trigger();
    let old = f.update().await?;
    f.app
        .editor
        .handlers
        .completions
        .event(CompletionEvent::Cancel);
    assert!(old.apply(&mut f.app.editor).is_none());
    f.trigger();
    let old = f.update().await?;
    f.app.editor.mode = Mode::Normal;
    assert!(old.apply(&mut f.app.editor).is_none());
    f.app.editor.mode = Mode::Insert;
    f.trigger();
    let old = f.update().await?;
    current!(f.app.editor)
        .1
        .set_path(Some(&dir.path().join("renamed.completion-test")));
    assert!(old.apply(&mut f.app.editor).is_none());
    f.trigger();
    let old = f.update().await?;
    let doc = current_ref!(f.app.editor).1.id();
    f.app.editor.new_file(Action::Replace);
    f.app.editor.switch(doc, Action::Replace);
    assert!(old.apply(&mut f.app.editor).is_none());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_ownership_and_handler_replacement_reject_queued_work() -> anyhow::Result<()> {
    let a = tempfile::tempdir()?;
    let b = tempfile::tempdir()?;
    let mut first = Fixture::new(a.path(), "😀 ap\n", &["alpha"]).await?;
    let mut second = Fixture::new(b.path(), "😀 ap\n", &["alpha"]).await?;
    first.trigger();
    first.response().await?(&mut second.app.editor);
    assert!(completion::next_update(&mut second.app.editor).is_none());
    first.trigger();
    let update = first.update().await?;
    assert!(update.apply(&mut second.app.editor).is_none());
    second.trigger();
    assert_eq!(second.items().await?.len(), 2);
    first.trigger();
    let old = first.response().await?;
    first.app.editor.handlers.completions = CompletionHandler::new(
        EditorCallbackSender::new(|_| async {}, |_| {}),
        &first.app.editor.config(),
    );
    old(&mut first.app.editor);
    assert!(completion::next_update(&mut first.app.editor).is_none());
    assert!(
        tokio::time::timeout(Duration::from_secs(5), first.callbacks.recv())
            .await?
            .is_none()
    );
    assert!(first.app.close().await.is_empty());
    assert!(second.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_preview_abort_accept_and_undo_keep_savepoints() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 ap\n", &["alpha"]).await?;
    f.show().await?;
    f.key("<C-n>").await?;
    assert_eq!(f.text(), "😀 apricot\n");
    f.key("<C-c>").await?;
    assert_eq!(f.text(), "😀 ap\n");
    f.show().await?;
    f.key("<C-n>").await?;
    f.key("<ret>").await?;
    assert_eq!(f.text(), "😀 apricot\n");
    f.key("<esc>").await?;
    f.key("u").await?;
    assert_eq!(f.text(), "😀 ap\n");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_change_restores_preview_in_original_document() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 ap\n", &["alpha"]).await?;
    f.show().await?;
    f.key("<C-n>").await?;
    let doc = current_ref!(f.app.editor).1.id();
    f.app.editor.new_file(Action::Replace);
    let update = completion::next_update(&mut f.app.editor).unwrap();
    f.app
        .handle_editor_event(EditorEvent::Completion(update))
        .await;
    assert_eq!(
        f.app.editor.document(doc).unwrap().text().to_string(),
        "😀 ap\n"
    );
    assert_eq!(f.text(), "\n");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn canceled_inflight_requests_and_late_providers_keep_their_session() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 ap\n", &["alpha", "beta"]).await?;
    // Let alpha provide the initial list, then release beta after that menu exists.
    let beta_gate = dir.path().join("beta-ready");
    std::fs::remove_file(&beta_gate)?;
    f.show().await?;
    assert_eq!(
        f.app.editor.handlers.completions.active_completions.len(),
        1
    );
    std::fs::write(&beta_gate, "ready")?;
    let old = f.update().await?;
    f.key("<C-c>").await?;
    assert!(old.apply(&mut f.app.editor).is_none());
    // Hold a request after the server has received it, then cancel before its reply.
    std::fs::remove_file(&f.response_gate)?;
    f.trigger();
    loop {
        let (scheduling, callback) =
            tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
                .await?
                .unwrap();
        callback(&mut f.app.editor);
        if scheduling {
            let update = completion::next_update(&mut f.app.editor).unwrap();
            assert!(matches!(
                update.apply(&mut f.app.editor),
                Some(CompletionChange::Started)
            ));
            break;
        }
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while std::fs::read_to_string(&f.log)
            .unwrap_or_default()
            .lines()
            .count()
            < 2
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    f.app
        .editor
        .handlers
        .completions
        .event(CompletionEvent::Cancel);
    f.trigger();
    std::fs::write(&f.response_gate, "ready")?;
    assert_eq!(f.items().await?.len(), 4);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn resolved_items_and_incomplete_refreshes_expire_with_the_menu() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 incomplete ap\n", &["alpha"]).await?;
    f.trigger();
    let mut items = f.items().await?;
    f.app.editor.last_completion = Some(view::editor::CompleteAction::Triggered);
    let mut resolver = completion::ResolveHandler::new();
    let CompletionItem::Lsp(item) = &mut items[0] else {
        panic!("expected LSP completion")
    };
    resolver.ensure_item_resolved(&mut f.app.editor, item);
    assert!(
        matches!(f.update().await?.apply(&mut f.app.editor), Some(CompletionChange::Resolved { item, .. }) if matches!(&*item, CompletionItem::Lsp(item) if item.resolved && item.item.documentation.is_some()))
    );
    completion::request_incomplete_completion_list(&mut f.app.editor);
    let old = f.update().await?;
    completion::request_incomplete_completion_list(&mut f.app.editor);
    assert!(old.apply(&mut f.app.editor).is_none());
    let old = f.update().await?;
    f.app.editor.handlers.completions.dismiss();
    assert!(old.apply(&mut f.app.editor).is_none());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_path_and_word_providers_work_without_lsp() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("apple.txt"), "")?;
    let text = "./ap\n";
    let mut f = Fixture::new(dir.path(), text, &[]).await?;
    let mut config = (*f.app.editor.config()).clone();
    config.lsp.enable = false;
    config.path_completion = true;
    f.app
        .handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    while completion::next_update(&mut f.app.editor).is_some() {}
    assert!(current_ref!(f.app.editor).1.path_completion_enabled());
    assert_eq!(f.app.editor.mode(), Mode::Insert);
    f.trigger();
    assert!(f
        .items()
        .await
        .context("path provider")?
        .iter()
        .any(|item| item.filter_text() == "apple.txt"));
    let mut config = (*f.app.editor.config()).clone();
    config.word_completion.enable = true;
    config.path_completion = false;
    f.app
        .handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    let path = dir.path().join("words.completion-test");
    std::fs::write(&path, "apple apricot ap\n")?;
    f.app.editor.open(&path, Action::Replace)?;
    f.app.editor.mode = Mode::Insert;
    let (view, doc) = current!(f.app.editor);
    doc.set_selection(view.id, Selection::point(doc.text().len_chars() - 1));
    while completion::next_update(&mut f.app.editor).is_some() {}
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.app.editor.handlers.word_index().matches("ap").len() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .context("word indexing")?;
    f.trigger();
    let items = f.items().await.context("word provider")?;
    assert!(items.iter().any(|item| item.filter_text() == "apple"));
    assert!(items.iter().any(|item| item.filter_text() == "apricot"));
    assert!(items.iter().all(|item| item.filter_text() != "ap"));
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_triggers_and_configuration_changes_keep_request_lifetimes() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "😀 ap.\n", &["alpha"]).await?;
    let mut config = (*f.app.editor.config()).clone();
    config.auto_completion = true;
    f.app
        .handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    completion::trigger_auto_completion(&f.app.editor, true);
    let update = f.update().await?;
    let requests = std::fs::read_to_string(&f.log)?;
    let request: serde_json::Value = serde_json::from_str(requests.lines().last().unwrap())?;
    assert_eq!(request["context"]["triggerKind"], 2);
    assert_eq!(request["context"]["triggerCharacter"], ".");
    let mut config = (*f.app.editor.config()).clone();
    config.auto_completion = false;
    f.app
        .handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    assert!(update.apply(&mut f.app.editor).is_none());
    while completion::next_update(&mut f.app.editor).is_some() {}
    f.trigger();
    assert_eq!(f.items().await?.len(), 2);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_insert_repeat_uses_the_original_request_savepoint() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), "ap\nap\n", &["alpha"]).await?;
    f.app.editor.mode = Mode::Normal;
    let (view, doc) = current!(f.app.editor);
    doc.set_selection(view.id, Selection::point(1));
    f.key("a").await?;
    f.show().await?;
    f.key("<C-n>").await?;
    f.key("<ret>").await?;
    f.key("<esc>").await?;
    assert_eq!(f.text(), "apricot\nap\n");
    let (view, doc) = current!(f.app.editor);
    doc.set_selection(view.id, Selection::point(9));
    f.key(".").await?;
    assert_eq!(f.text(), "apricot\napricot\n");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

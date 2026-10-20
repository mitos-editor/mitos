use std::{path::PathBuf, time::Duration};

use anyhow::Context as _;
use editor_core::{Selection, Transaction};
use serde_json::Value;
use term::application::Application;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use view::{
    callbacks::EditorCallback,
    current, current_ref,
    editor::Action,
    features::code_lenses::{self, DocumentCodeLensesHandler, ResolvedCodeLenses},
    DocumentId,
};

use super::helpers::{
    callbacks,
    lsp::{self, Gate, ServerConfig},
    run_event_loop_until_idle, test_config, AppBuilder,
};

struct Fixture {
    app: Application,
    callbacks: mpsc::UnboundedReceiver<(bool, EditorCallback)>,
    logs: Vec<PathBuf>,
    doc: DocumentId,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new(names: &[&str], flags: &[&str]) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let mut servers = String::new();
        let mut gates = Vec::new();
        let mut logs = Vec::new();
        for name in names {
            let gate = Gate::new(dir.path().join(format!("{name}.initialize-ready")));
            let mut server =
                ServerConfig::feature(name, "--code-lenses", dir.path()).initialize_gate(&gate);
            for flag in flags {
                server = server.arg(flag);
            }
            servers.push_str(&server.toml());
            logs.push(lsp::log_path(dir.path(), name));
            gates.push(gate);
        }
        let loader = lsp::syntax_loader("lens-test", "lens-test", names, &servers);
        let mut config = test_config();
        config.editor.lsp.enable = true;
        let (sender, callbacks) = callbacks::unbounded(|blocking, callback| (blocking, callback));
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(loader)
            .with_handler_setup(move |handlers, _| {
                handlers.code_lenses = DocumentCodeLensesHandler::new(sender);
            })
            .build()?;
        let path = dir.path().join("document.lens-test");
        std::fs::write(&path, "😀 original\n")?;
        app.editor.open(&path, Action::Replace)?;
        let doc = current_ref!(app.editor).1.id();
        let mut fixture = Self {
            app,
            callbacks,
            logs,
            doc,
            _dir: dir,
        };
        for gate in gates {
            gate.release()?;
            lsp::initialize(&mut fixture.app, 1).await?;
            let callback = fixture.response().await?;
            callback(&mut fixture.app.editor);
        }
        Ok(fixture)
    }

    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (blocking, callback) = self
                    .callbacks
                    .recv()
                    .await
                    .context("callback queue closed")?;
                if blocking {
                    callback(&mut self.app.editor)
                } else {
                    return Ok(callback);
                }
            }
        })
        .await?
    }

    async fn resolve(&mut self) -> ResolvedCodeLenses {
        code_lenses::resolve_code_lenses(&mut self.app.editor, self.doc, 0)
            .unwrap()
            .await
            .unwrap()
    }

    fn messages(&self, server: usize) -> Vec<Value> {
        std::fs::read_to_string(&self.logs[server])
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn edit(&mut self, text: &str) {
        let (view, doc) = current!(self.app.editor);
        let transaction = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        assert!(view::editing::apply(doc, &transaction, view.id));
    }

    async fn keys(&mut self, input: &str) -> anyhow::Result<()> {
        for key in ui_core::input::parse_macro(input)? {
            #[cfg(not(windows))]
            let event = termina::event::Event::Key(termina::event::KeyEvent::from(key));
            #[cfg(windows)]
            let event = crossterm::event::Event::Key(crossterm::event::KeyEvent::from(key));
            self.app.handle_terminal_events(Ok(event)).await;
        }
        // Menu input can be consumed without EditorView resetting the idle timer.
        self.app.editor.reset_idle_timer();
        tokio::time::timeout(
            Duration::from_secs(10),
            run_event_loop_until_idle(&mut self.app),
        )
        .await?;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn multiple_servers_resolve_and_cache_their_own_lenses() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(&["alpha", "beta"], &[]).await?;
    let doc = fixture.app.editor.document(fixture.doc).unwrap();
    // Invalid ranges are dropped, UTF-16 column 3 maps to character offset 2.
    assert_eq!(doc.code_lenses().len(), 8);
    assert!(doc.code_lenses().iter().all(|lens| lens.char_idx == 2));
    let resolved = fixture.resolve().await;
    assert!(resolved.is_current(&fixture.app.editor));
    assert_eq!(
        resolved
            .commands
            .iter()
            .map(|lens| lens.command.title.as_str())
            .collect::<Vec<_>>(),
        [
            "alpha ready",
            "alpha resolved",
            "beta ready",
            "beta resolved"
        ]
    );
    for command in &resolved.commands {
        let server = fixture
            .app
            .editor
            .language_servers
            .get_by_id(command.language_server_id)
            .unwrap();
        assert!(command.command.title.starts_with(server.name()));
    }
    resolved.cache(&mut fixture.app.editor);
    let cached = fixture
        .app
        .editor
        .document(fixture.doc)
        .unwrap()
        .code_lenses();
    assert_eq!(
        cached
            .iter()
            .filter(|lens| lens.lens.command.is_some())
            .count(),
        4
    );
    // Resolution preserves opaque server data and does not replace neighboring lenses.
    for server in 0..2 {
        let messages = fixture.messages(server);
        let requests: Vec<_> = messages
            .iter()
            .filter(|msg| msg["method"] == "codeLens/resolve")
            .collect();
        assert_eq!(requests.len(), 3);
        assert!(requests
            .iter()
            .all(|msg| msg["params"]["data"]["server"] == ["alpha", "beta"][server]));
    }
    let again = fixture.resolve().await;
    assert_eq!(again.commands.len(), 4);
    // Only lenses still lacking commands need another resolve request.
    for server in 0..2 {
        assert_eq!(
            fixture
                .messages(server)
                .iter()
                .filter(|msg| msg["method"] == "codeLens/resolve")
                .count(),
            5
        );
    }
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unsupported_resolution_is_skipped_and_null_response_clears_cache() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(&["alpha"], &["--no-resolve"]).await?;
    let resolved = fixture.resolve().await;
    assert_eq!(resolved.commands.len(), 1);
    assert!(!fixture
        .messages(0)
        .iter()
        .any(|msg| msg["method"] == "codeLens/resolve"));
    fixture.edit("😀 no lenses\n");
    let callback = fixture.response().await?;
    callback(&mut fixture.app.editor);
    assert!(fixture
        .app
        .editor
        .document(fixture.doc)
        .unwrap()
        .code_lenses()
        .is_empty());
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn mapped_gutter_positions_follow_newlines_and_resolution_rejects_old_snapshots(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new(&["alpha"], &[]).await?;
    let resolved = fixture.resolve().await;
    let (view, doc) = current!(fixture.app.editor);
    let transaction = Transaction::insert(doc.text(), doc.selection(view.id), "\n".into());
    assert!(view::editing::apply(doc, &transaction, view.id));
    assert_eq!(doc.text().char_to_line(doc.code_lenses()[0].char_idx), 1);
    assert!(!resolved.is_current(&fixture.app.editor));
    assert!(code_lenses::resolve_code_lenses(&mut fixture.app.editor, fixture.doc, 1).is_none());
    fixture
        .app
        .handle_editor_event(view::editor::EditorEvent::Redraw)
        .await;
    let (view, doc) = current_ref!(fixture.app.editor);
    let x = view.area.x;
    let y = view.inner_area(doc).y;
    let buffer = fixture.app.terminal_backend().buffer();
    assert_eq!(buffer[(x, y)].symbol(), " ");
    assert_eq!(buffer[(x, y + 1)].symbol(), "▶");
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_requests_are_acknowledged_and_invalidate_queued_resolution() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new(&["alpha"], &["--refresh"]).await?;
    let resolved = fixture.resolve().await;
    let (server, call) = tokio::time::timeout(
        Duration::from_secs(10),
        fixture.app.editor.language_servers.incoming.next(),
    )
    .await?
    .unwrap();
    assert!(
        matches!(&call, lsp_client::Call::MethodCall(call) if call.method == "workspace/codeLens/refresh")
    );
    fixture
        .app
        .handle_language_server_message(call, server)
        .await;
    assert!(!resolved.is_current(&fixture.app.editor));
    let callback = fixture.response().await?;
    callback(&mut fixture.app.editor);
    // A barrier request makes sure the server has received the refresh reply.
    fixture.resolve().await;
    assert!(fixture
        .messages(0)
        .iter()
        .any(|msg| msg["id"] == "lens-refresh"
            && msg["result"].is_null()
            && msg.get("error").is_none()));
    assert_eq!(
        fixture
            .messages(0)
            .iter()
            .filter(|msg| msg["method"] == "textDocument/codeLens")
            .count(),
        2
    );
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn disabling_lenses_discards_queued_results_and_enabling_requests_them_again(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new(&["alpha"], &[]).await?;
    let resolved = fixture.resolve().await;
    let server = resolved.commands[0].language_server_id;
    code_lenses::refresh_for_language_server(&mut fixture.app.editor, server);
    let queued = fixture.response().await?;
    let mut config = fixture.app.editor.config().clone();
    config.lsp.code_lenses.enable = false;
    fixture
        .app
        .handle_config_events(view::editor::ConfigEvent::Update(Box::new(config.clone())));
    queued(&mut fixture.app.editor);
    assert!(!resolved.is_current(&fixture.app.editor));
    assert!(fixture
        .app
        .editor
        .document(fixture.doc)
        .unwrap()
        .code_lenses()
        .is_empty());
    assert!(code_lenses::resolve_code_lenses(&mut fixture.app.editor, fixture.doc, 0).is_none());
    config.lsp.code_lenses.enable = true;
    fixture
        .app
        .handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    let fresh = fixture.response().await?;
    fresh(&mut fixture.app.editor);
    assert_eq!(
        fixture
            .app
            .editor
            .document(fixture.doc)
            .unwrap()
            .code_lenses()
            .len(),
        4
    );
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn renaming_document_invalidates_resolution_and_refreshes_its_lenses() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(&["alpha"], &[]).await?;
    let resolved = fixture.resolve().await;
    let path = fixture._dir.path().join("renamed.lens-test");
    fixture.app.editor.set_doc_path(fixture.doc, &path);
    assert!(!resolved.is_current(&fixture.app.editor));
    assert!(fixture
        .app
        .editor
        .document(fixture.doc)
        .unwrap()
        .code_lenses()
        .is_empty());
    let fresh = fixture.response().await?;
    fresh(&mut fixture.app.editor);
    let resolved = fixture.resolve().await;
    assert_eq!(resolved.commands.len(), 2);
    let requests: Vec<_> = fixture
        .messages(0)
        .into_iter()
        .filter(|msg| msg["method"] == "textDocument/codeLens")
        .collect();
    assert_eq!(requests.len(), 2);
    assert!(requests[1]["params"]["textDocument"]["uri"]
        .as_str()
        .unwrap()
        .ends_with("/renamed.lens-test"));
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_debounce_does_not_request_server_ranges_for_temporary_text() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(&["alpha"], &[]).await?;
    fixture.edit("😀 changed\n");
    // Hold the debounce dispatch until a completion preview changes the local text.
    let (blocking, schedule) =
        tokio::time::timeout(Duration::from_secs(10), fixture.callbacks.recv())
            .await?
            .unwrap();
    assert!(blocking);
    let (view, doc) = current!(fixture.app.editor);
    let original = doc.text().clone();
    let transaction = Transaction::insert(doc.text(), doc.selection(view.id), "preview".into());
    assert!(view::editing::apply_temporary(doc, &transaction, view.id));
    schedule(&mut fixture.app.editor);
    assert!(fixture.callbacks.try_recv().is_err());
    assert_eq!(
        fixture
            .messages(0)
            .iter()
            .filter(|msg| msg["method"] == "textDocument/codeLens")
            .count(),
        1
    );
    // Restoring server-visible text schedules a fresh request, without didChange.
    let (view, doc) = current!(fixture.app.editor);
    let restore = Transaction::change(
        doc.text(),
        [(0, doc.text().len_chars(), Some(original.to_string().into()))].into_iter(),
    );
    assert!(view::editing::apply_temporary(doc, &restore, view.id));
    let fresh = fixture.response().await?;
    fresh(&mut fixture.app.editor);
    assert_eq!(fixture.resolve().await.commands.len(), 2);
    assert_eq!(
        fixture
            .messages(0)
            .iter()
            .filter(|msg| msg["method"] == "textDocument/didChange")
            .count(),
        1
    );
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn space_l_menu_executes_commands_on_the_selected_server() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(&["alpha", "beta"], &[]).await?;
    fixture.keys("<space>l").await?;
    fixture.keys("<down><down><ret>").await?;
    // Commands are sent asynchronously; wait for the selected server to receive it.
    let command = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(command) = fixture
                .messages(1)
                .into_iter()
                .find(|message| message["method"] == "workspace/executeCommand")
            {
                break command;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .context("selected server did not receive the code-lens command")?;
    assert!(!fixture
        .messages(0)
        .iter()
        .any(|msg| msg["method"] == "workspace/executeCommand"));
    assert_eq!(command["params"]["command"], "fixture.lens");
    assert_eq!(command["params"]["arguments"][0], "beta");
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn menu_does_not_execute_after_its_document_selection_changes() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(&["alpha"], &[]).await?;
    fixture.keys("<space>l").await?;
    let (view, doc) = current!(fixture.app.editor);
    doc.set_selection(view.id, Selection::point(3));
    fixture.keys("<ret>").await?;
    fixture.resolve().await;
    assert!(!fixture
        .messages(0)
        .iter()
        .any(|msg| msg["method"] == "workspace/executeCommand"));
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn visible_titles_resolve_automatically_and_repeated_frames_reuse_attempts(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new(&["alpha"], &[]).await?;
    // A normal input frame schedules resolution through the application prelude.
    fixture.keys("l").await?;
    let title = fixture.response().await?;
    fixture
        .app
        .job_sender()
        .send_callback(term::job::Callback::Editor(title))
        .await;
    fixture.keys("").await?;
    let doc = fixture.app.editor.document(fixture.doc).unwrap();
    assert_eq!(
        doc.code_lenses()
            .iter()
            .filter(|lens| lens.lens.command.is_some())
            .count(),
        2
    );
    assert_eq!(
        doc.code_lenses()[1].lens.command.as_ref().unwrap().title,
        "alpha resolved"
    );
    for _ in 0..5 {
        code_lenses::prepare_visible_code_lenses(&mut fixture.app.editor);
    }
    // A transport barrier ensures all preceding resolution requests were logged.
    let doc = fixture.app.editor.document(fixture.doc).unwrap();
    let server = doc.language_servers().next().unwrap();
    server.code_lens(doc.identifier()).unwrap().await?;
    assert_eq!(
        fixture
            .messages(0)
            .iter()
            .filter(|msg| msg["method"] == "codeLens/resolve")
            .count(),
        3
    );
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_title_resolution_follows_the_visible_lines_when_scrolling() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new(&["alpha"], &["--per-line"]).await?;
    fixture.edit(&"😀 line\n".repeat(200));
    let fresh = fixture.response().await?;
    fresh(&mut fixture.app.editor);
    let (view, doc) = current!(fixture.app.editor);
    view.area = view::graphics::Rect::new(0, 0, 50, 4);
    let visible = view.inner_height(doc);
    code_lenses::prepare_visible_code_lenses(&mut fixture.app.editor);
    for _ in 0..visible {
        let title = fixture.response().await?;
        title(&mut fixture.app.editor);
    }
    let doc = fixture.app.editor.document(fixture.doc).unwrap();
    assert!(doc.code_lenses()[..visible]
        .iter()
        .all(|lens| lens.lens.command.is_some()));
    assert!(doc.code_lenses()[visible..]
        .iter()
        .all(|lens| lens.lens.command.is_none()));
    let (view, doc) = current!(fixture.app.editor);
    doc.set_view_offset(
        view.id,
        view::view::ViewPosition {
            anchor: doc.text().line_to_char(100),
            ..Default::default()
        },
    );
    code_lenses::prepare_visible_code_lenses(&mut fixture.app.editor);
    for _ in 0..visible {
        let title = fixture.response().await?;
        title(&mut fixture.app.editor);
    }
    let doc = fixture.app.editor.document(fixture.doc).unwrap();
    assert!(doc.code_lenses()[100..100 + visible]
        .iter()
        .all(|lens| lens.lens.command.is_some()));
    assert!(doc.code_lenses()[visible..100]
        .iter()
        .all(|lens| lens.lens.command.is_none()));
    assert_eq!(
        fixture
            .messages(0)
            .iter()
            .filter(|msg| msg["method"] == "codeLens/resolve")
            .count(),
        visible * 2
    );
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn soft_wrapping_keeps_hidden_document_lines_unresolved() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(&["alpha"], &["--per-line"]).await?;
    fixture.edit(&format!("{}\n😀 hidden\n", "long line ".repeat(100)));
    let fresh = fixture.response().await?;
    fresh(&mut fixture.app.editor);
    let mut config = fixture.app.editor.config().clone();
    config.soft_wrap.enable = Some(true);
    fixture
        .app
        .handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    let fresh = fixture.response().await?;
    fresh(&mut fixture.app.editor);
    let (view, _) = current!(fixture.app.editor);
    view.area = view::graphics::Rect::new(0, 0, 30, 4);
    code_lenses::prepare_visible_code_lenses(&mut fixture.app.editor);
    let title = fixture.response().await?;
    title(&mut fixture.app.editor);
    let doc = fixture.app.editor.document(fixture.doc).unwrap();
    assert!(doc.code_lenses()[0].lens.command.is_some());
    assert!(doc.code_lenses()[1].lens.command.is_none());
    assert_eq!(
        fixture
            .messages(0)
            .iter()
            .filter(|msg| msg["method"] == "codeLens/resolve")
            .count(),
        1
    );
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn automatic_titles_queued_before_an_edit_cannot_publish() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(&["alpha"], &[]).await?;
    code_lenses::prepare_visible_code_lenses(&mut fixture.app.editor);
    let stale = fixture.response().await?;
    fixture.edit("😀 changed\n");
    stale(&mut fixture.app.editor);
    let doc = fixture.app.editor.document(fixture.doc).unwrap();
    assert!(doc.code_lenses()[1].lens.command.is_none());
    let fresh = fixture.response().await?;
    fresh(&mut fixture.app.editor);
    code_lenses::prepare_visible_code_lenses(&mut fixture.app.editor);
    let title = fixture.response().await?;
    title(&mut fixture.app.editor);
    assert!(fixture
        .app
        .editor
        .document(fixture.doc)
        .unwrap()
        .code_lenses()[1]
        .lens
        .command
        .is_some());
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn title_requests_are_limited_before_the_client_sends_them() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(&["alpha"], &["--per-line", "--hold-resolves"]).await?;
    fixture.edit(&"😀 line\n".repeat(20));
    let fresh = fixture.response().await?;
    fresh(&mut fixture.app.editor);
    let (view, doc) = current!(fixture.app.editor);
    view.area = view::graphics::Rect::new(0, 0, 50, 10);
    let visible = view.inner_height(doc);
    assert!(visible > 4);
    code_lenses::prepare_visible_code_lenses(&mut fixture.app.editor);
    tokio::time::timeout(Duration::from_secs(10), async {
        while fixture
            .messages(0)
            .iter()
            .filter(|msg| msg["method"] == "codeLens/resolve")
            .count()
            < 4
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    let doc = fixture.app.editor.document(fixture.doc).unwrap();
    let server = doc.language_servers().next().unwrap();
    server.code_lens(doc.identifier()).unwrap().await?;
    // The fake server keeps reading while responses are held: it has received
    // only four requests, despite more visible lenses waiting to be resolved.
    assert_eq!(
        fixture
            .messages(0)
            .iter()
            .filter(|msg| msg["method"] == "codeLens/resolve")
            .count(),
        4
    );
    server
        .command(lsp_client::lsp::Command {
            title: "release".into(),
            command: "fixture.release-lenses".into(),
            arguments: None,
        })
        .unwrap()
        .await?;
    for _ in 0..visible {
        let title = fixture.response().await?;
        title(&mut fixture.app.editor);
    }
    assert_eq!(
        fixture
            .messages(0)
            .iter()
            .filter(|msg| msg["method"] == "codeLens/resolve")
            .count(),
        visible
    );
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

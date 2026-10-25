use std::{path::PathBuf, time::Duration};

use editor_core::Transaction;
use lsp_client::lsp;
use serde_json::{json, Value};
use term::application::Application;
use view::{current, current_ref, editor::Action};

use super::helpers::{
    lsp::{self as fixture, Gate, ServerConfig},
    run_event_loop_until_idle, test_config, AppBuilder,
};

struct Fixture {
    app: Application,
    log: PathBuf,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new(flags: &[&str]) -> anyhow::Result<Self> {
        Self::with_callback_sender(flags, None).await
    }

    async fn with_callback_sender(
        flags: &[&str],
        sender: Option<view::callbacks::EditorCallbackSender>,
    ) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let gate = Gate::new(dir.path().join("initialize-ready"));
        let mut server =
            ServerConfig::feature("alpha", "--interactive", dir.path()).initialize_gate(&gate);
        for flag in flags {
            server = server.arg(flag);
        }
        let loader = fixture::syntax_loader(
            "interactive-test",
            "interactive-test",
            &["alpha"],
            &server.toml(),
        );
        let mut config = test_config();
        config.editor.lsp.enable = true;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(loader)
            .with_handler_setup(move |handlers, config| {
                if let Some(sender) = sender {
                    *handlers = view::handlers::Handlers::new(config, sender);
                }
            })
            .build()?;
        let path = dir.path().join("document.interactive-test");
        std::fs::write(&path, "original\n")?;
        app.editor.open(&path, Action::Replace)?;
        gate.release()?;
        fixture::initialize(&mut app, 1).await?;
        Ok(Self {
            app,
            log: fixture::log_path(dir.path(), "alpha"),
            _dir: dir,
        })
    }

    fn server(&self) -> lsp_client::LanguageServerId {
        self.app.editor.language_servers().next().unwrap().id()
    }
    fn messages(&self, method: &str) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|message| message["method"] == method)
            .collect()
    }
    fn screen(&self) -> String {
        self.app
            .terminal_backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }
    async fn pump(&mut self) -> anyhow::Result<()> {
        self.app.editor.reset_idle_timer();
        tokio::time::timeout(
            Duration::from_secs(10),
            run_event_loop_until_idle(&mut self.app),
        )
        .await?;
        Ok(())
    }
    async fn start(&mut self, scenario: &str) -> anyhow::Result<()> {
        let command = lsp::Command {
            title: "Interactive".into(),
            command: "fixture.interactive".into(),
            arguments: Some(vec![
                json!(scenario),
                json!(current_ref!(self.app.editor).1.url().unwrap()),
            ]),
        };
        self.app.editor.execute_lsp_command(command, self.server());
        self.pump().await
    }
    async fn keys(&mut self, input: &str) -> anyhow::Result<()> {
        for key in ui_core::input::parse_macro(input)? {
            #[cfg(not(windows))]
            let event = termina::event::Event::Key(termina::event::KeyEvent::from(key));
            #[cfg(windows)]
            let event = crossterm::event::Event::Key(crossterm::event::KeyEvent::from(key));
            self.app.handle_terminal_events(Ok(event)).await;
        }
        self.pump().await
    }
    async fn barrier(&mut self) -> anyhow::Result<()> {
        self.app
            .editor
            .language_server_by_id(self.server())
            .unwrap()
            .did_change_configuration(json!({"barrier":true}));
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.messages("workspace/didChangeConfiguration").is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        Ok(())
    }
    async fn gopls_action(&mut self, title: &str) -> anyhow::Result<()> {
        let doc = current_ref!(self.app.editor).1;
        let context = view::action::LspActionContext::new(doc);
        let server = self.server();
        let future = self
            .app
            .editor
            .language_server_by_id(server)
            .unwrap()
            .code_actions(
                doc.identifier(),
                if title.contains("interface") {
                    lsp::Range::new(lsp::Position::new(2, 6), lsp::Position::new(2, 6))
                } else {
                    lsp::Range::new(lsp::Position::new(2, 0), lsp::Position::new(4, 1))
                },
                lsp::CodeActionContext {
                    diagnostics: vec![],
                    only: None,
                    trigger_kind: Some(lsp::CodeActionTriggerKind::INVOKED),
                },
            )
            .unwrap();
        tokio::pin!(future);
        let actions = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                tokio::select! {
                    result = &mut future => break result,
                    _ = run_event_loop_until_idle(&mut self.app) => {}
                }
            }
        })
        .await??
        .unwrap_or_default();
        let action = actions
            .into_iter()
            .find(|action| match action {
                lsp::CodeActionOrCommand::Command(command) => command.title.contains(title),
                lsp::CodeActionOrCommand::CodeAction(action) => action.title.contains(title),
            })
            .ok_or_else(|| anyhow::anyhow!("gopls did not offer {title}"))?;
        view::action::Action::lsp(context, server, action).execute(&mut self.app.editor);
        self.pump().await
    }

    async fn close(&mut self) {
        assert!(self.app.close().await.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_rounds_retry_only_errors_and_execute_the_returned_command(
) -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    let initialize = f.messages("initialize");
    assert_eq!(
        initialize[0]["params"]["capabilities"]["experimental"]["interactiveResolve"]["inputTypes"],
        json!(["string", "bool", "number", "enum", "lazyEnum", "file", "list"])
    );
    f.start("rounds").await?;
    assert!(f.screen().contains("Name: seed"));
    f.keys("<C-u>bad<ret><ret>").await?;
    assert!(f.screen().contains("Choose another name"));
    assert_eq!(f.messages("command/resolve").len(), 2);
    f.keys("<C-u>good<ret>").await?;
    assert!(f.screen().contains("Confirm"));
    let resolves = f.messages("command/resolve");
    assert_eq!(
        resolves[2]["params"]["formAnswers"],
        json!([
        {"id":"case","value":"snake"},{"id":"name","value":"good"}])
    );
    assert!(f.messages("workspace/executeCommand").is_empty());
    f.keys("<ret>").await?;
    let executed = f.messages("workspace/executeCommand");
    assert_eq!(executed.len(), 1);
    assert_eq!(executed[0]["params"]["command"], "fixture.resolved");
    assert_eq!(executed[0]["params"]["arguments"][0], "confirmed");
    assert_eq!(
        executed[0]["params"]["formAnswers"],
        json!([{"id":"confirm","value":true}])
    );
    assert!(executed[0]["params"].get("formFields").is_none());
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_all_static_types_and_nested_lists_keep_json_types() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    f.start("all").await?;
    f.keys("<ret><ret><ret><ret><ret>new.go<ret>").await?;
    assert!(f.screen().contains("Nested list"));
    f.keys("<ret><ret>7<ret>Done<ret>Done<ret>").await?;
    let execute = f.messages("workspace/executeCommand");
    assert_eq!(execute.len(), 1, "screen: {}", f.screen());
    let answers = &execute[0]["params"]["formAnswers"];
    let uri = lsp::Url::from_file_path(f._dir.path().join("new.go")).unwrap();
    assert_eq!(
        *answers,
        json!([
        {"id":"text","value":"hello"},{"id":"optional","value":null},
        {"id":"bool","value":false},{"id":"number","value":2.5},
        {"id":"enum","value":"wire"},{"id":"file","value":uri},
        {"id":"list","value":[[7]]}])
    );
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_dynamic_search_preserves_opaque_config_and_returns_option_value(
) -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    f.start("search").await?;
    // A dynamic result arrives after the debounce; drive its frontend callback.
    f.pump().await?;
    assert!(f.screen().contains("Readable interface"));
    // The server may return semantic matches whose labels do not contain the query.
    f.keys("semanticAlias").await?;
    assert!(f.screen().contains("Readable interface"));
    f.keys("<ret>").await?;
    let queries = f.messages("interactive/listEnum");
    assert_eq!(
        queries[0]["params"],
        json!({"source":"symbols","config":{"opaque":[1,false,"test"]},"query":""})
    );
    assert_eq!(queries[1]["params"]["query"], "semanticAlias");
    assert_eq!(
        f.messages("workspace/executeCommand")[0]["params"]["formAnswers"],
        json!([{"id":"interface","value":"pkg.Type"}])
    );
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_escape_cancels_forms_and_pending_dynamic_search() -> anyhow::Result<()> {
    let mut f = Fixture::new(&["--hold-search"]).await?;
    f.start("search").await?;
    f.pump().await?;
    assert_eq!(f.messages("interactive/listEnum").len(), 1);
    // Enter without a result must not dismiss the question.
    f.keys("<ret>pkg").await?;
    assert_eq!(f.messages("interactive/listEnum").len(), 2);
    assert_eq!(
        f.messages("interactive/listEnum")[1]["params"]["query"],
        "pkg"
    );
    f.keys("<esc>").await?;
    f.barrier().await?;
    assert!(f.messages("workspace/executeCommand").is_empty());
    assert!(!f.messages("$/cancelRequest").is_empty());
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_dynamic_enum_defaults_survive_the_first_search() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    f.start("search").await?;
    f.pump().await?;
    f.keys("<ret>").await?;
    assert_eq!(
        f.messages("workspace/executeCommand")[0]["params"]["formAnswers"],
        json!([{"id":"interface","value":"default.Interface"}])
    );
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_document_edits_cancel_an_outstanding_resolution_rpc() -> anyhow::Result<()> {
    let mut f = Fixture::new(&["--hold-resolve"]).await?;
    f.start("edit").await?;
    assert_eq!(f.messages("command/resolve").len(), 1);
    let (view, doc) = current!(f.app.editor);
    let transaction = Transaction::insert(doc.text(), doc.selection(view.id), "changed".into());
    view::editing::apply(doc, &transaction, view.id)?;
    f.pump().await?;
    f.barrier().await?;
    assert!(!f.messages("$/cancelRequest").is_empty());
    assert!(f.messages("workspace/executeCommand").is_empty());
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_escape_cancels_an_outstanding_resolution_rpc() -> anyhow::Result<()> {
    let mut f = Fixture::new(&["--hold-resolve"]).await?;
    f.start("edit").await?;
    f.keys("<esc>").await?;
    f.barrier().await?;
    assert_eq!(f.messages("$/cancelRequest").len(), 1);
    assert!(f.messages("workspace/executeCommand").is_empty());
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_document_changes_and_server_stop_dismiss_forms() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    f.start("edit").await?;
    assert!(f.screen().contains("Text:"));
    let (view, doc) = current!(f.app.editor);
    let transaction = Transaction::insert(doc.text(), doc.selection(view.id), "changed".into());
    view::editing::apply(doc, &transaction, view.id)?;
    f.pump().await?;
    assert!(!f.screen().contains("Text:"));
    f.start("edit").await?;
    assert!(f.screen().contains("Text:"));
    f.app.editor.handle_language_server_exit(f.server());
    f.pump().await?;
    assert!(!f.screen().contains("Text:"));
    assert!(f.messages("workspace/executeCommand").is_empty());
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_capability_negotiation_and_ready_commands_preserve_legacy_execution(
) -> anyhow::Result<()> {
    for flags in [&["--no-interactive"][..], &["--standard"][..], &[][..]] {
        let mut f = Fixture::new(flags).await?;
        f.start("ready").await?;
        assert_eq!(f.messages("workspace/executeCommand").len(), 1);
        assert_eq!(
            f.messages("command/resolve").len(),
            usize::from(!flags.contains(&"--no-interactive"))
        );
        f.close().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_invalid_forms_are_reported_without_executing() -> anyhow::Result<()> {
    for scenario in ["unsupported", "duplicate"] {
        let mut f = Fixture::new(&[]).await?;
        f.start(scenario).await?;
        assert!(f
            .app
            .editor
            .status_msg
            .as_ref()
            .unwrap()
            .0
            .contains("Invalid interactive form"));
        assert!(f.messages("workspace/executeCommand").is_empty());
        f.close().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_workspace_edits_apply_after_input_and_remain_undoable() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    f.start("edit").await?;
    f.keys("answer<ret>").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.text().to_string(),
        "done original\n"
    );
    let (_, doc) = current!(f.app.editor);
    assert!(doc.undo());
    assert_eq!(doc.text().to_string(), "original\n");
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_list_defaults_can_be_edited_and_removed() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    f.start("list").await?;
    f.keys("Edit item 1<ret><C-u>updated<ret>Remove item 2<ret>Done<ret>")
        .await?;
    assert_eq!(
        f.messages("workspace/executeCommand")[0]["params"]["formAnswers"],
        json!([{"id":"list","value":["updated"]}])
    );
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_errors_are_visible_and_do_not_fall_back_to_execution() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    f.start("resolve-error").await?;
    assert!(f
        .app
        .editor
        .status_msg
        .as_ref()
        .unwrap()
        .0
        .contains("Failed to resolve LSP command"));
    assert!(f.messages("workspace/executeCommand").is_empty());
    f.start("execute-error").await?;
    f.keys("answer<ret>").await?;
    assert!(f
        .app
        .editor
        .status_msg
        .as_ref()
        .unwrap()
        .0
        .contains("Failed to execute LSP command"));
    assert_eq!(f.messages("workspace/executeCommand").len(), 1);
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_actions_keep_the_originating_document_when_focus_moves() -> anyhow::Result<()>
{
    let mut f = Fixture::new(&[]).await?;
    let doc = current_ref!(f.app.editor).1;
    let original = doc.id();
    let action = view::action::Action::lsp(
        view::action::LspActionContext::new(doc),
        f.server(),
        lsp::CodeActionOrCommand::Command(lsp::Command {
            title: "Action".into(),
            command: "fixture.interactive".into(),
            arguments: Some(vec![json!("edit"), json!(doc.url().unwrap())]),
        }),
    );
    f.app.editor.new_file(Action::VerticalSplit);
    action.execute(&mut f.app.editor);
    f.pump().await?;
    assert!(f.screen().contains("Text:"));
    f.keys("answer<ret>").await?;
    assert_eq!(
        f.app.editor.document(original).unwrap().text().to_string(),
        "done original\n"
    );
    assert_eq!(current_ref!(f.app.editor).1.text().to_string(), "\n");
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_invalid_numeric_input_stays_open_for_correction() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    f.start("all").await?;
    f.keys("<ret><ret><ret><C-u>oops<ret>").await?;
    assert!(f.screen().contains("Number: oops"));
    assert!(f.messages("workspace/executeCommand").is_empty());
    f.keys("<C-u>1.25<ret><ret>wrong.txt<ret>").await?;
    assert!(f.screen().contains("File: wrong.txt"));
    f.keys("<C-u>new.go<ret>Done<ret>").await?;
    assert_eq!(f.messages("workspace/executeCommand").len(), 1);
    f.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_disabling_lsp_dismisses_the_visible_form() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    f.start("edit").await?;
    assert!(f.screen().contains("Text:"));
    let mut config = f.app.editor.config().clone();
    config.lsp.enable = false;
    f.app
        .handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    f.pump().await?;
    assert!(!f.screen().contains("Text:"));
    assert!(f.messages("workspace/executeCommand").is_empty());
    f.close().await;
    Ok(())
}

// Capture the editor callback before it publishes either a form or a ready command.
async fn queued_resolution(
    scenario: &str,
) -> anyhow::Result<(
    Fixture,
    view::callbacks::EditorCallback,
    tokio::sync::mpsc::UnboundedReceiver<view::callbacks::EditorCallback>,
)> {
    let (sender, mut callbacks) = super::helpers::callbacks::unbounded(|_, callback| callback);
    let mut f = Fixture::with_callback_sender(&[], Some(sender)).await?;
    // Finish background document setup before observing the command's reply.
    while let Ok(callback) = callbacks.try_recv() {
        callback(&mut f.app.editor);
    }
    f.start(scenario).await?;
    let callback = tokio::time::timeout(Duration::from_secs(10), callbacks.recv())
        .await?
        .ok_or_else(|| anyhow::anyhow!("interactive callback queue closed"))?;
    Ok((f, callback, callbacks))
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_queued_replies_reject_changed_or_closed_documents() -> anyhow::Result<()> {
    for scenario in ["edit", "ready"] {
        for close in [false, true] {
            let (mut f, callback, _callbacks) = queued_resolution(scenario).await?;
            if close {
                let doc = current_ref!(f.app.editor).1.id();
                f.app.editor.new_file(Action::Replace);
                assert!(f.app.editor.close_document(doc, true).is_ok());
            } else {
                let (view, doc) = current!(f.app.editor);
                let change = Transaction::insert(doc.text(), doc.selection(view.id), "new".into());
                view::editing::apply(doc, &change, view.id)?;
            }
            callback(&mut f.app.editor);
            assert!(view::features::interactive::next_request(&mut f.app.editor).is_none());
            f.barrier().await?;
            assert!(f.messages("workspace/executeCommand").is_empty());
            f.close().await;
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_queued_replies_reject_cancellation_and_newer_commands() -> anyhow::Result<()> {
    for scenario in ["edit", "ready"] {
        let (mut f, callback, mut callbacks) = queued_resolution(scenario).await?;
        assert!(view::features::interactive::cancel(&mut f.app.editor));
        f.start("edit").await?;
        let newer = tokio::time::timeout(Duration::from_secs(10), callbacks.recv())
            .await?
            .expect("newer resolution reply");
        newer(&mut f.app.editor);
        f.pump().await?;
        assert!(f.screen().contains("Text:"));
        callback(&mut f.app.editor);
        assert!(view::features::interactive::next_request(&mut f.app.editor).is_none());
        f.barrier().await?;
        assert!(f.messages("workspace/executeCommand").is_empty());
        f.close().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_new_commands_replace_visible_forms() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    f.start("edit").await?;
    assert!(f.screen().contains("Text:"));
    f.start("rounds").await?;
    assert!(f.screen().contains("Name: seed"));
    assert!(!f.screen().contains("Text:"));
    f.keys("<esc>").await?;
    assert!(f.messages("workspace/executeCommand").is_empty());
    f.close().await;
    Ok(())
}

/// Run explicitly with MITOS_TEST_GOPLS pointing to a gopls v0.23.0+ executable.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires an installed gopls v0.23.0+ and Go toolchain"]
async fn interactive_real_gopls_struct_tags_and_interfaces() -> anyhow::Result<()> {
    let gopls = std::env::var("MITOS_TEST_GOPLS")?;
    let dir = tempfile::tempdir()?;
    let cache = dir.path().join("cache");
    std::fs::create_dir_all(&cache)?;
    std::fs::write(
        dir.path().join("go.mod"),
        "module example.com/interactive\n\ngo 1.23\n",
    )?;
    let path = dir.path().join("main.go");
    std::fs::write(
        &path,
        "package example\n\ntype Person struct {\n FirstName string\n}\n\ntype Greeter interface { Greet() string }\n",
    )?;
    let quoted = |s: &str| toml::Value::String(s.to_owned()).to_string();
    let loader = super::helpers::test_syntax_loader(Some(format!(
        r#"
        [language-server.gopls]
        command = {}
        environment = {{ GOCACHE = {}, GOPLSCACHE = {}, GOPROXY = "off", GOTOOLCHAIN = "local" }}
        [[language]]
        name = "go"
        scope = "source.go"
        file-types = ["go"]
        roots = ["go.mod"]
        language-servers = ["gopls"]
    "#,
        quoted(&gopls),
        quoted(&cache.to_string_lossy()),
        quoted(&cache.to_string_lossy())
    )));
    let mut config = test_config();
    config.editor.lsp.enable = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(loader)
        .build()?;
    app.editor.open(&path, Action::Replace)?;
    tokio::time::timeout(Duration::from_secs(30), async {
        while !app
            .editor
            .language_servers()
            .any(|server| server.is_initialized())
        {
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    let mut f = Fixture {
        app,
        log: dir.path().join("unused"),
        _dir: dir,
    };
    f.gopls_action("Add struct tags").await?;
    assert!(f.screen().contains("json"), "{}", f.screen());
    f.keys("<C-u>json,yaml<ret>snake<ret>").await?;
    let text = current_ref!(f.app.editor).1.text().to_string();
    assert!(text.contains("json:\"first_name\""), "{text}");
    assert!(text.contains("yaml:\"first_name\""), "{text}");
    f.gopls_action("implement an interface").await?;
    f.keys("Greeter").await?;
    assert!(f.screen().contains("Greeter"), "{}", f.screen());
    f.keys("<ret>").await?;
    let text = current_ref!(f.app.editor).1.text().to_string();
    assert!(text.contains(" Greet() string {"), "{text}");
    f.close().await;
    Ok(())
}

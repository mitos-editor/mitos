use std::{path::PathBuf, time::Duration};

use super::Fixture;
use lsp_client::{
    jsonrpc,
    test_support::{read_log, Gate},
    Call, LanguageServerId,
};
use serde_json::{json, Value};
use view::{
    current_ref,
    handlers::lsp::{DeferredReply, HostAction},
};

struct Server {
    fixture: Fixture,
    _logs: tempfile::TempDir,
    log: PathBuf,
    gate: Gate,
    id: LanguageServerId,
}

impl Server {
    fn new() -> anyhow::Result<Self> {
        let logs = tempfile::tempdir()?;
        let log = logs.path().join("server.jsonl");
        let gate = Gate::new(logs.path().join("ready"));
        let command = toml::Value::String(env!("CARGO_BIN_EXE_mitos-view-test-lsp").into());
        let args = toml::Value::try_from(vec![
            "--lifecycle",
            "headless",
            log.to_str().unwrap(),
            gate.path().to_str().unwrap(),
        ])?;
        let languages = format!(
            r#"
[language-server.headless]
command = {command}
args = {args}
config = {{ dispatch = "configured" }}
[[language]]
name = "words"
scope = "text.words"
roots = []
file-types = ["words"]
language-servers = ["headless"]
"#
        );
        let fixture = Fixture::with_config(
            "😀 words\n",
            &languages,
            loader::syntax::Resources::default(),
            |config| config.lsp.enable = true,
        )?;
        let id = fixture.editor.language_servers().next().unwrap().id();
        Ok(Self {
            fixture,
            _logs: logs,
            log,
            gate,
            id,
        })
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        self.gate.release()?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (id, call) = self
                    .fixture
                    .editor
                    .next_language_server_message()
                    .await
                    .unwrap();
                let initialized =
                    matches!(&call, Call::Notification(n) if n.method == "initialized");
                assert!(self
                    .fixture
                    .editor
                    .handle_language_server_message(call, id)
                    .await
                    .is_none());
                if initialized {
                    break;
                }
            }
        })
        .await?;
        Ok(())
    }

    async fn request(&mut self, id: &str, method: &str, params: Value) -> Option<HostAction> {
        self.fixture
            .editor
            .handle_language_server_message(request(id, method, params), self.id)
            .await
    }

    async fn prompt(&mut self, id: &str) -> DeferredReply {
        let Some(HostAction::ShowMessageRequest {
            reply,
            actions,
            message,
        }) = self
            .request(
                id,
                "window/showMessageRequest",
                json!({"type": 3, "message": "Choose", "actions": [{"title": "Yes"}]}),
            )
            .await
        else {
            panic!("expected prompt");
        };
        assert_eq!(message, "Choose");
        assert_eq!(actions[0].title, "Yes");
        reply
    }

    async fn flush(&self) -> anyhow::Result<Vec<Value>> {
        self.fixture
            .editor
            .language_server_by_id(self.id)
            .unwrap()
            .shutdown()
            .await?;
        read_log(&self.log)
    }
}

fn request(id: &str, method: &str, params: Value) -> Call {
    serde_json::from_value(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
        .unwrap()
}

fn notification(method: &str, params: Value) -> Call {
    serde_json::from_value(json!({"jsonrpc": "2.0", "method": method, "params": params})).unwrap()
}

#[tokio::test]
async fn ordinary_requests_and_protocol_errors_are_replied_to_without_a_frontend(
) -> anyhow::Result<()> {
    let mut s = Server::new()?;
    s.initialize().await?;
    for (id, method, params) in [
        (
            "configuration",
            "workspace/configuration",
            json!({"items": [{"section": "dispatch"}]}),
        ),
        ("folders", "workspace/workspaceFolders", Value::Null),
        ("unknown", "unknown/method", Value::Null),
        ("malformed", "workspace/applyEdit", json!({})),
        (
            "register",
            "client/registerCapability",
            json!({"registrations": []}),
        ),
        (
            "unregister",
            "client/unregisterCapability",
            json!({"unregisterations": []}),
        ),
        ("diagnostics", "workspace/diagnostic/refresh", Value::Null),
        ("tokens", "workspace/semanticTokens/refresh", Value::Null),
        ("folding", "workspace/foldingRange/refresh", Value::Null),
        ("lenses", "workspace/codeLens/refresh", Value::Null),
        ("inline", "workspace/inlineValue/refresh", Value::Null),
    ] {
        assert!(s.request(id, method, params).await.is_none());
    }
    assert!(s
        .fixture
        .editor
        .handle_language_server_message(
            Call::Invalid {
                id: jsonrpc::Id::Str("invalid".into())
            },
            s.id
        )
        .await
        .is_none());
    let message = s
        .request(
            "message",
            "window/showMessageRequest",
            json!({"type": 3, "message": "Info"}),
        )
        .await;
    assert!(matches!(message, Some(HostAction::ShowMessage(_))));
    let records = s.flush().await?;
    let reply = |id: &str| records.iter().find(|record| record["id"] == id).unwrap();
    assert_eq!(reply("configuration")["result"], json!(["configured"]));
    assert_eq!(reply("unknown")["error"]["code"], -32601);
    assert_eq!(reply("malformed")["error"]["code"], -32700);
    assert_eq!(reply("invalid")["error"]["code"], -32600);
    for id in [
        "register",
        "unregister",
        "diagnostics",
        "tokens",
        "folding",
        "lenses",
        "inline",
        "message",
    ] {
        assert_eq!(reply(id).get("result"), Some(&Value::Null));
    }
    let folders = s
        .fixture
        .editor
        .language_server_by_id(s.id)
        .unwrap()
        .workspace_folders()
        .clone();
    assert_eq!(
        reply("folders")["result"],
        if folders.is_empty() {
            Value::Null
        } else {
            json!(folders)
        }
    );
    Ok(())
}

#[tokio::test]
async fn configuration_requests_can_arrive_before_initialization() -> anyhow::Result<()> {
    let mut s = Server::new()?;
    // An early initialized event must not publish readiness before wire initialization.
    s.fixture
        .editor
        .handle_language_server_message(notification("initialized", Value::Null), s.id)
        .await;
    assert!(!s
        .fixture
        .editor
        .language_server_by_id(s.id)
        .unwrap()
        .is_initialized());
    assert!(s
        .request(
            "pending",
            "workspace/configuration",
            json!({"items": [{"section": "dispatch"}]})
        )
        .await
        .is_none());
    assert!(s
        .request("folders", "workspace/workspaceFolders", Value::Null)
        .await
        .is_none());
    assert!(s
        .request(
            "pending-document",
            "window/showDocument",
            json!({"uri": "file:///tmp/pending.words"})
        )
        .await
        .is_none());
    assert!(s
        .request("pending-edit", "workspace/applyEdit", json!({"edit": {}}))
        .await
        .is_none());
    s.initialize().await?;
    let records = s.flush().await?;
    assert_eq!(
        records.iter().find(|r| r["id"] == "pending").unwrap()["result"],
        json!(["configured"])
    );
    assert!(records
        .iter()
        .any(|r| r["id"] == "folders" && r.get("result").is_some()));
    for id in ["pending-document", "pending-edit"] {
        assert_eq!(
            records.iter().find(|r| r["id"] == id).unwrap()["error"]["code"],
            -32600
        );
    }
    Ok(())
}

#[tokio::test]
async fn deferred_replies_finish_once_and_dropping_is_silent() -> anyhow::Result<()> {
    let mut s = Server::new()?;
    s.initialize().await?;
    s.prompt("accept")
        .await
        .complete(&s.fixture.editor, Ok(json!({"title": "Yes"})));
    s.prompt("abort")
        .await
        .complete(&s.fixture.editor, Ok(Value::Null));
    drop(s.prompt("drop").await);
    let records = s.flush().await?;
    for id in ["accept", "abort"] {
        assert_eq!(records.iter().filter(|r| r["id"] == id).count(), 1);
    }
    assert_eq!(
        records.iter().find(|r| r["id"] == "accept").unwrap()["result"],
        json!({"title": "Yes"})
    );
    assert_eq!(
        records.iter().find(|r| r["id"] == "abort").unwrap()["result"],
        Value::Null
    );
    assert!(!records.iter().any(|r| r["id"] == "drop"));
    Ok(())
}

#[tokio::test]
async fn deferred_replies_cannot_target_another_editor_or_exited_client() -> anyhow::Result<()> {
    let mut original = Server::new()?;
    let mut other = Server::new()?;
    original.initialize().await?;
    other.initialize().await?;
    assert_eq!(original.id, other.id);
    original
        .prompt("other-editor")
        .await
        .complete(&other.fixture.editor, Ok(Value::Null));
    let reply = original.prompt("after-exit").await;
    let client = original
        .fixture
        .editor
        .language_servers()
        .next()
        .unwrap()
        .clone();
    original
        .fixture
        .editor
        .handle_language_server_exit(original.id);
    reply.complete(&original.fixture.editor, Ok(Value::Null));
    // Keep the old process alive to prove silence rather than relying on a closed pipe.
    client.shutdown().await?;
    let original_records = read_log(&original.log)?;
    let other_records = other.flush().await?;
    for records in [original_records, other_records] {
        assert!(!records.iter().any(|r| ["other-editor", "after-exit"]
            .iter()
            .any(|id| r["id"] == *id)));
    }
    Ok(())
}

#[tokio::test]
async fn progress_and_document_requests_return_only_concrete_host_actions() -> anyhow::Result<()> {
    let mut s = Server::new()?;
    s.initialize().await?;
    assert!(s
        .request(
            "progress",
            "window/workDoneProgress/create",
            json!({"token": "index"})
        )
        .await
        .is_none());
    let action = s
        .fixture
        .editor
        .handle_language_server_message(
            notification(
                "$/progress",
                json!({"token": "index", "value": {"kind": "begin", "title": "Indexing"}}),
            ),
            s.id,
        )
        .await;
    assert!(matches!(action, Some(HostAction::Progress(_))));
    assert!(s.fixture.editor.language_server_is_progressing(s.id));
    let before = current_ref!(s.fixture.editor).1.id();
    let Some(HostAction::ShowDocument {
        params,
        offset_encoding,
        reply,
    }) = s
        .request(
            "document",
            "window/showDocument",
            json!({"uri": "https://example.com", "external": true, "takeFocus": true}),
        )
        .await
    else {
        panic!("expected document action");
    };
    assert_eq!(offset_encoding, lsp_client::OffsetEncoding::Utf16);
    assert_eq!(params.external, Some(true));
    assert_eq!(current_ref!(s.fixture.editor).1.id(), before);
    drop(reply);
    let reply = s.prompt("stopped").await;
    s.fixture.editor.stop_language_servers(before, &[])?;
    reply.complete(&s.fixture.editor, Ok(Value::Null));
    assert!(!s.fixture.editor.language_server_is_progressing(s.id));
    assert!(s
        .fixture
        .editor
        .handle_language_server_message(
            notification(
                "$/progress",
                json!({"token": "late", "value": {"kind": "begin", "title": "Late"}})
            ),
            s.id
        )
        .await
        .is_none());
    Ok(())
}

#[tokio::test]
async fn empty_workspace_folders_reply_with_null() -> anyhow::Result<()> {
    let mut s = Server::new()?;
    s.initialize().await?;
    s.fixture
        .editor
        .language_server_by_id(s.id)
        .unwrap()
        .workspace_folders()
        .clear();
    assert!(s
        .request("empty-folders", "workspace/workspaceFolders", Value::Null)
        .await
        .is_none());
    let records = s.flush().await?;
    assert_eq!(
        records
            .iter()
            .find(|r| r["id"] == "empty-folders")
            .unwrap()
            .get("result"),
        Some(&Value::Null)
    );
    Ok(())
}

#[tokio::test]
async fn deferred_replies_do_not_follow_a_restarted_server() -> anyhow::Result<()> {
    let mut s = Server::new()?;
    s.initialize().await?;
    let reply = s.prompt("old-instance").await;
    let old_id = s.id;
    let document = current_ref!(s.fixture.editor).1.id();
    s.fixture.editor.restart_language_servers(document, &[])?;
    s.id = s.fixture.editor.language_servers().next().unwrap().id();
    assert_ne!(s.id, old_id);
    s.initialize().await?;
    reply.complete(&s.fixture.editor, Ok(Value::Null));
    assert!(!s.flush().await?.iter().any(|r| r["id"] == "old-instance"));
    Ok(())
}

#[tokio::test]
async fn deferred_replies_are_suppressed_during_exit_hooks_before_registry_removal(
) -> anyhow::Result<()> {
    use std::sync::{Arc, Mutex};

    use futures_util::FutureExt as _;
    use view::events::LanguageServerExited;

    let mut s = Server::new()?;
    s.initialize().await?;
    let reply = Arc::new(Mutex::new(Some(s.prompt("during-stop").await)));
    let pending = reply.clone();
    let client = s.fixture.editor.language_servers().next().unwrap().clone();
    let server_id = s.id;
    let message = || {
        notification(
            "window/showMessage",
            json!({"type": 3, "message": "Still active"}),
        )
    };
    assert!(matches!(
        s.fixture
            .editor
            .handle_language_server_message(message(), server_id)
            .await,
        Some(HostAction::ShowMessage(_))
    ));
    let _registration = event::register_owned_hook!(s.fixture.editor.events() =>
        move |event: &mut LanguageServerExited<'_>| {
            assert_eq!(event.server_id, server_id);
            assert!(event.editor.language_server_by_id(server_id).is_some());
            // The registry is private. Its inactive state is observable through
            // dispatch: a registered active client would return ShowMessage here.
            assert!(event.editor.handle_language_server_message(message(), server_id)
                .now_or_never().expect("stopped-client dispatch must finish immediately").is_none());
            pending.lock().unwrap().take().unwrap().complete(event.editor, Ok(Value::Null));
            Ok(())
        }
    );
    s.fixture.editor.handle_language_server_exit(server_id);
    assert!(
        reply.lock().unwrap().is_none(),
        "exit hook must consume the reply"
    );
    assert!(s.fixture.editor.language_server_by_id(server_id).is_none());
    // Retain the process and flush its input, proving that suppression happened
    // while registered rather than because removal closed the connection.
    client.shutdown().await?;
    assert!(!read_log(&s.log)?
        .iter()
        .any(|record| record["id"] == "during-stop"));
    Ok(())
}

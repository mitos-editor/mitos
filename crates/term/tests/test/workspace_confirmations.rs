//! Consent binds workspace edits and commands to the original server and affected state.
use super::helpers::{
    keys,
    lsp::{self as fixture_lsp, Gate, ServerConfig},
    test_config, AppBuilder,
};
use editor_core::Transaction;
use lsp_client::{lsp, UriExt as _};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};
use view::{
    action::{Action, LspActionContext},
    current, current_ref,
    editor::{Action as OpenAction, EditorEvent},
    workspace_edit::confirmation::ConfirmationRequest,
};

#[tokio::test(flavor = "multi_thread")]
async fn workspace_confirmations_are_fifo_and_cannot_be_resolved_by_another_editor(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let other_dir = tempfile::tempdir()?;
    let mut f = fixture(dir.path()).await?;
    let mut other = fixture(other_dir.path()).await?;
    let mut receivers = Vec::new();
    for label in ["First", "Second"] {
        let mut edit = edit(&f);
        edit.change_annotations
            .as_mut()
            .unwrap()
            .get_mut("confirm")
            .unwrap()
            .label = label.into();
        let (tx, rx) = tokio::sync::oneshot::channel();
        f.app
            .editor
            .request_workspace_edit(f.server("alpha"), edit, None, move |_, result| {
                let _ = tx.send(result);
            });
        receivers.push(rx);
    }
    let first = prompt(&mut f).await?;
    assert_eq!(first.annotations()[0].label, "First");
    first.resolve(&mut other.app.editor, true);
    assert!(first.is_current(&f.app.editor));
    first.resolve(&mut f.app.editor, false);
    let second = prompt(&mut f).await?;
    assert_eq!(second.annotations()[0].label, "Second");
    second.resolve(&mut f.app.editor, true);
    assert!(!receivers.remove(0).await??);
    assert!(receivers.remove(0).await??);
    assert_eq!(current_ref!(other.app.editor).1.text(), "😀 original\n");
    assert!(f.app.close().await.is_empty());
    assert!(other.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_confirmation_batches_preserve_annotation_namespaces_and_atomic_undo(
) -> anyhow::Result<()> {
    for accepted in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut f = fixture(dir.path()).await?;
        let mut first = serde_json::to_value(edit(&f))?;
        first["documentChanges"][0]["edits"][0]["range"]["end"]["character"] = json!(7);
        first["documentChanges"][0]["edits"][0]["newText"] = json!("ORIG");
        first["changeAnnotations"]["confirm"]["label"] = json!("First action");
        let mut second = serde_json::to_value(edit(&f))?;
        second["documentChanges"][0]["edits"][0]["range"]["start"]["character"] = json!(7);
        second["documentChanges"][0]["edits"][0]["newText"] = json!("INAL");
        second["changeAnnotations"]["confirm"]["label"] = json!("Second action");
        let edits = vec![
            serde_json::from_value(first)?,
            serde_json::from_value(second)?,
        ];
        let context = LspActionContext::new(current_ref!(f.app.editor).1);
        let (tx, rx) = tokio::sync::oneshot::channel();
        f.app.editor.request_code_action_edits(
            f.server("alpha"),
            edits,
            Some(context),
            move |_, result| {
                let _ = tx.send(result);
            },
        );
        let request = prompt(&mut f).await?;
        assert_eq!(
            request
                .annotations()
                .iter()
                .map(|annotation| annotation.label.as_str())
                .collect::<Vec<_>>(),
            ["First action", "Second action"]
        );
        request.resolve(&mut f.app.editor, accepted);
        assert_eq!(rx.await??, accepted);
        assert_eq!(
            current_ref!(f.app.editor).1.text().to_string(),
            if accepted {
                "😀 ORIGINAL\n"
            } else {
                "😀 original\n"
            }
        );
        if accepted {
            let (view, doc) = current!(f.app.editor);
            assert!(view::editing::undo(doc, view.id));
            assert_eq!(doc.text(), "😀 original\n");
        }
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

fn edit(f: &Fixture) -> lsp::WorkspaceEdit {
    serde_json::from_value(json!({
        "documentChanges":[{"textDocument":{"uri":f.uri().to_url().unwrap()},"edits":[{"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":11}},"newText":"changed","annotationId":"confirm"}]}],
        "changeAnnotations":{"confirm":{"label":"Update references", "description":"This changes the selected symbol", "needsConfirmation":true}, "unused":{"label":"Unused", "needsConfirmation":true}}
    })).unwrap()
}
async fn prompt(f: &mut Fixture) -> anyhow::Result<ConfirmationRequest> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = f.app.editor.wait_event().await;
            if let EditorEvent::WorkspaceEditConfirmation(request) = event {
                return request;
            }
            f.app.handle_editor_event(event).await;
        }
    })
    .await
    .map_err(Into::into)
}
struct Fixture {
    app: term::application::TestApplication,
    logs: Vec<std::path::PathBuf>,
}
impl Fixture {
    fn uri(&self) -> editor_core::Uri {
        current_ref!(self.app.editor).1.uri().unwrap()
    }
    fn server(&self, name: &str) -> lsp_client::LanguageServerId {
        self.app
            .editor
            .language_servers()
            .find(|server| server.name() == name)
            .unwrap()
            .id()
    }
}
async fn fixture(dir: &Path) -> anyhow::Result<Fixture> {
    let gate = Gate::new(dir.join("initialize-ready"));
    let server = ServerConfig::feature("alpha", "--action-execution", dir)
        .initialize_gate(&gate)
        .arg("--no-resolve");
    let mut config = test_config();
    config.editor.lsp.enable = true;
    config.editor.completion.auto_trigger = false;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(fixture_lsp::syntax_loader(
            "lifecycle-test",
            "lifecycle-test",
            &["alpha"],
            &server.toml(),
        ))
        .build()?;
    let path = dir.join("document.lifecycle-test");
    std::fs::write(&path, "😀 original\n")?;
    app.editor.open(&path, OpenAction::Replace)?;
    gate.release()?;
    fixture_lsp::initialize(&mut app, 1).await?;
    Ok(Fixture {
        app,
        logs: vec![fixture_lsp::log_path(dir, "alpha")],
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_confirmation_accepts_or_declines_before_reply_and_edits() -> anyhow::Result<()> {
    for accepted in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut f = fixture(dir.path()).await?;
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        let edit = edit(&f);
        f.app.editor.request_workspace_edit_from_server(
            f.server("alpha"),
            lsp::ApplyWorkspaceEditParams { label: None, edit },
            move |_, response| {
                let _ = tx.send(response);
            },
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(current_ref!(f.app.editor).1.text(), "😀 original\n");
        let request = prompt(&mut f).await?;
        assert_eq!(request.annotations().len(), 1);
        assert_eq!(request.annotations()[0].label, "Update references");
        assert_eq!(
            request.annotations()[0].description.as_deref(),
            Some("This changes the selected symbol")
        );
        request.resolve(&mut f.app.editor, accepted);
        request.resolve(&mut f.app.editor, !accepted);
        assert_eq!(rx.await??.applied, accepted);
        assert_eq!(
            current_ref!(f.app.editor).1.text().to_string(),
            if accepted {
                "😀 changed\n"
            } else {
                "😀 original\n"
            }
        );
        if accepted {
            let (view, doc) = current!(f.app.editor);
            assert!(view::editing::undo(doc, view.id));
            assert_eq!(doc.text(), "😀 original\n");
        }
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_confirmation_rejects_stale_text_uri_server_and_disk_state() -> anyhow::Result<()>
{
    for change in ["text", "uri", "server", "disk", "open", "closed"] {
        let dir = tempfile::tempdir()?;
        let mut f = fixture(dir.path()).await?;
        let target = dir.path().join("target.txt");
        std::fs::write(&target, "target\n")?;
        let mut edit = edit(&f);
        let lsp::DocumentChanges::Edits(changes) = edit.document_changes.as_mut().unwrap() else {
            unreachable!()
        };
        changes.push(serde_json::from_value(json!({"textDocument":{"uri":lsp::Uri::from_file_path(&target).unwrap()}, "edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}, "newText":"prefix "}]}))?);
        let server = f.server("alpha");
        let document = current_ref!(f.app.editor).1.id();
        let original = f.uri().as_path().unwrap().to_owned();
        let (tx, rx) = tokio::sync::oneshot::channel();
        f.app
            .editor
            .request_workspace_edit(server, edit, None, move |_, result| {
                let _ = tx.send(result);
            });
        let request = prompt(&mut f).await?;
        match change {
            "text" => {
                let doc = f.app.editor.document_mut(document).unwrap();
                let change =
                    Transaction::change(doc.text(), [(0, 0, Some("new ".into()))].into_iter());
                doc.apply(change.changes())?;
            }
            "uri" => f
                .app
                .editor
                .set_doc_path(document, &dir.path().join("renamed.lifecycle-test")),
            "server" => f.app.editor.handle_language_server_exit(server),
            "disk" => std::fs::write(&target, "externally changed\n")?,
            "open" => {
                f.app.editor.open(&target, OpenAction::Load)?;
            }
            "closed" => {
                assert!(f.app.editor.close_document(document, true).is_ok());
            }
            _ => unreachable!(),
        }
        request.resolve(&mut f.app.editor, true);
        assert!(rx.await?.is_err(), "{change}");
        assert_eq!(std::fs::read_to_string(original)?, "😀 original\n");
        assert_eq!(
            std::fs::read_to_string(&target)?,
            if change == "disk" {
                "externally changed\n"
            } else {
                "target\n"
            },
            "{change}"
        );
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_confirmation_does_not_run_code_action_commands_when_declined(
) -> anyhow::Result<()> {
    for accepted in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut f = fixture(dir.path()).await?;
        let server = f.server("alpha");
        let action = lsp::CodeAction {
            title: "Confirm and run".into(),
            edit: Some(edit(&f)),
            command: Some(lsp::Command {
                title: "Run".into(),
                command: "fixture.command".into(),
                arguments: None,
            }),
            ..Default::default()
        };
        let context = LspActionContext::new(current_ref!(f.app.editor).1);
        Action::lsp(
            context,
            server,
            lsp::CodeActionOrCommand::CodeAction(action),
        )
        .execute(&mut f.app.editor);
        let request = prompt(&mut f).await?;
        request.resolve(&mut f.app.editor, accepted);
        if accepted {
            fixture_lsp::wait_message(&f.logs[0], "workspace/executeCommand").await?;
        } else {
            // A harmless fixture command is a transport barrier: any incorrectly
            // executed dependent command must have been logged before its reply.
            f.app
                .editor
                .language_server_by_id(server)
                .unwrap()
                .command(lsp::Command {
                    title: "Test barrier".into(),
                    command: "fixture.command".into(),
                    arguments: Some(vec![json!("barrier")]),
                })
                .unwrap()
                .await?;
            let commands: Vec<_> = fixture_lsp::read_log(&f.logs[0])?
                .into_iter()
                .filter(|message| message["method"] == "workspace/executeCommand")
                .collect();
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0]["params"]["arguments"], json!(["barrier"]));
        }
        assert_eq!(
            current_ref!(f.app.editor).1.text().to_string(),
            if accepted {
                "😀 changed\n"
            } else {
                "😀 original\n"
            }
        );
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_confirmation_terminal_prompts_cancel_by_default_and_reply_after_decision(
) -> anyhow::Result<()> {
    for input in ["<ret>", "<esc>", "<down><ret>"] {
        let dir = tempfile::tempdir()?;
        let mut f = fixture(dir.path()).await?;
        let server = f.server("alpha");
        let id = "annotated-edit";
        let call = serde_json::from_value(
            json!({"jsonrpc":"2.0","id":id,"method":"workspace/applyEdit","params":{"edit":edit(&f)}}),
        )?;
        f.app.handle_language_server_message(call, server).await;
        let request = prompt(&mut f).await?;
        f.app
            .handle_editor_event(EditorEvent::WorkspaceEditConfirmation(request))
            .await;
        keys(&mut f.app, input).await?;
        let reply: Value = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(reply) = fixture_lsp::read_log(&f.logs[0])
                    .unwrap_or_default()
                    .into_iter()
                    .find(|message| message["id"] == id)
                {
                    return reply;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        assert_eq!(reply["result"]["applied"], input == "<down><ret>");
        assert_eq!(
            current_ref!(f.app.editor).1.text().to_string(),
            if input == "<down><ret>" {
                "😀 changed\n"
            } else {
                "😀 original\n"
            }
        );
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn workspace_confirmation_guards_symlink_targets_and_open_alias_buffers() -> anyhow::Result<()>
{
    for external in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut f = fixture(dir.path()).await?;
        let actual = dir.path().join("real.txt");
        let alias = dir.path().join("alias.txt");
        std::fs::write(&actual, "target\n")?;
        std::os::unix::fs::symlink(&actual, &alias)?;
        let id = f.app.editor.open(&actual, OpenAction::Load)?;
        let mut edit = edit(&f);
        let lsp::DocumentChanges::Edits(changes) = edit.document_changes.as_mut().unwrap() else {
            unreachable!()
        };
        changes.push(serde_json::from_value(json!({"textDocument":{"uri":lsp::Uri::from_file_path(&alias).unwrap()}, "edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}, "newText":"prefix "}]}))?);
        let (tx, rx) = tokio::sync::oneshot::channel();
        f.app
            .editor
            .request_workspace_edit(f.server("alpha"), edit, None, move |_, result| {
                let _ = tx.send(result);
            });
        let request = prompt(&mut f).await?;
        if external {
            std::fs::write(&actual, "new external text\n")?;
        } else {
            let doc = f.app.editor.document_mut(id).unwrap();
            let tx = Transaction::change(doc.text(), [(0, 0, Some("new ".into()))].into_iter());
            doc.apply(tx.changes())?;
        }
        request.resolve(&mut f.app.editor, true);
        assert!(rx.await?.is_err());
        assert_eq!(current_ref!(f.app.editor).1.text(), "😀 original\n");
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

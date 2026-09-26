use std::{path::Path, time::Duration};

use lsp_client::{jsonrpc::ErrorCode, lsp};
use serde_json::{json, Value};
use view::current;
use view::file_watcher::Config;

use super::helpers::lsp::Fixture;

async fn logged(path: &Path, predicate: impl Fn(&Value) -> bool) -> anyhow::Result<Vec<Value>> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let messages: Vec<Value> = std::fs::read_to_string(path)?
                .split_inclusive('\n')
                .filter(|line| line.ends_with('\n'))
                .map(serde_json::from_str)
                .collect::<Result<_, _>>()?;
            if messages.iter().any(&predicate) {
                return anyhow::Ok(messages);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?
}

fn edit(f: &Fixture, version: i32, text: &str) -> Value {
    json!({"textDocument": {"uri": f.uri().to_url().unwrap(), "version": version},
        "edits": [{"range": {"start": {"line": 0, "character": 3},
            "end": {"line": 0, "character": 11}}, "newText": text}]})
}

#[tokio::test(flavor = "multi_thread")]
async fn configuration_preserves_order_sections_nulls_and_server_scope() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_config(dir.path(), &["alpha", "beta"], |name| {
        (name == "alpha")
            .then(|| "{ nested = { enabled = false, value = 42 }, list = [1, 2] }".into())
    })?;
    f.initialize().await?;
    let params = serde_json::from_value(json!({"items": [
        {}, {"section": ""}, {"section": "nested.value"},
        {"section": "nested.enabled", "scopeUri": "file:///elsewhere"},
        {"section": "missing"}, {"section": "nested.value.child"},
        {"section": "list"}, {"section": "nested.value"}
    ]}))?;
    let config = json!({"nested": {"enabled": false, "value": 42}, "list": [1, 2]});
    assert_eq!(
        json!(f
            .app
            .editor
            .language_server_by_id(f.server("alpha"))
            .unwrap()
            .configuration(&params)),
        json!([config, config, 42, false, null, null, [1, 2], 42])
    );
    assert_eq!(
        json!(f
            .app
            .editor
            .language_server_by_id(f.server("beta"))
            .unwrap()
            .configuration(&params)),
        json!([null, null, null, null, null, null, null, null])
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_edits_validate_server_use_utf16_and_report_partial_failure() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha"])?;
    let id = f.server("alpha");
    let params = json!({"edit": {"documentChanges": [edit(&f, 0, "updated!")]}});
    let error = f
        .app
        .editor
        .handle_workspace_edit(id, serde_json::from_value(params.clone())?)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(
        error.message,
        "Server must be initialized to request workspace edits"
    );
    assert_eq!(current!(f.app.editor).1.text().to_string(), "😀 original\n");
    f.initialize().await?;
    let response = f
        .app
        .editor
        .handle_workspace_edit(id, serde_json::from_value(params)?)?;
    assert!(response.applied);
    assert_eq!(response.failure_reason, None);
    assert_eq!(response.failed_change, None);
    let (view, doc) = current!(f.app.editor);
    assert_eq!(doc.text().to_string(), "😀 updated!\n");
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "😀 original\n");
    let version = doc.version();
    // Existing edits are not transactional: retain the first edit when the second is stale.
    let params = json!({"edit": {"documentChanges": [edit(&f, version, "retained"), edit(&f, version, "stale!!!")]}});
    let response = f
        .app
        .editor
        .handle_workspace_edit(id, serde_json::from_value(params)?)?;
    assert!(!response.applied);
    assert_eq!(response.failed_change, Some(1));
    assert_eq!(
        response.failure_reason.as_deref(),
        Some("document has changed")
    );
    assert_eq!(current!(f.app.editor).1.text().to_string(), "😀 retained\n");
    f.app.editor.handle_language_server_exit(id);
    assert_eq!(
        f.app
            .editor
            .handle_workspace_edit(id, serde_json::from_value(json!({"edit": {}}))?)
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

fn registration(id: &str, watchers: Value) -> Value {
    json!({"id": id, "method": "workspace/didChangeWatchedFiles", "registerOptions": {"watchers": watchers}})
}

#[tokio::test(flavor = "multi_thread")]
async fn registrations_add_relative_roots_and_unregister_only_the_named_server_interest(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let uri_root = tempfile::tempdir()?;
    let folder_root = tempfile::tempdir()?;
    let uri_path = uri_root.path().canonicalize()?;
    let folder_path = folder_root.path().canonicalize()?;
    let uri = lsp::Url::from_directory_path(&uri_path).unwrap();
    let folder = lsp::Url::from_directory_path(&folder_path).unwrap();
    let mut f = Fixture::new(dir.path(), &["alpha", "beta"])?;
    f.initialize().await?;
    f.app.editor.file_watcher.reload(&Config {
        enable: true,
        hidden: false,
        max_depth: Some(1),
        ..Config::default()
    });
    let alpha = f.server("alpha");
    let beta = f.server("beta");
    let watches = json!([
        {"globPattern": {"baseUri": uri, "pattern": "*.watched"}, "kind": 2},
        {"globPattern": {"baseUri": {"uri": folder, "name": "other"}, "pattern": "*.watched"}, "kind": 2}
    ]);
    for server in [alpha, beta] {
        f.app.editor.register_language_server_capabilities(server, serde_json::from_value(json!({"registrations": [
            {"id": "unsupported", "method": "textDocument/hover"},
            {"id": "absent", "method": "workspace/didChangeWatchedFiles"},
            {"id": "malformed", "method": "workspace/didChangeWatchedFiles", "registerOptions": {"watchers": false}},
            registration("shared-id", watches.clone()),
            registration("barrier", json!([{"globPattern": {"baseUri": uri, "pattern": "*.barrier"}}]))
        ]}))?);
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while !f
            .app
            .editor
            .file_watcher
            .is_watching(&uri_path.join("first.watched"))
            || !f
                .app
                .editor
                .file_watcher
                .is_watching(&folder_path.join("second.watched"))
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    let handler = &f.app.editor.language_servers.file_event_handler;
    handler.file_changed(uri_path.join("first.watched"), lsp::FileChangeType::CHANGED);
    handler.file_changed(
        folder_path.join("second.watched"),
        lsp::FileChangeType::CHANGED,
    );
    let second = lsp::Url::from_file_path(folder_path.join("second.watched")).unwrap();
    for log in &f.logs {
        logged(log, |m| m["params"]["changes"][0]["uri"] == second.as_str()).await?;
    }
    f.app.editor.unregister_language_server_capabilities(
        alpha,
        serde_json::from_value(json!({"unregisterations": [
            {"id": "shared-id", "method": "workspace/didChangeWatchedFiles"},
            {"id": "unsupported", "method": "textDocument/hover"}
        ]}))?,
    );
    let handler = &f.app.editor.language_servers.file_event_handler;
    handler.file_changed(uri_path.join("after.watched"), lsp::FileChangeType::CHANGED);
    handler.file_changed(uri_path.join("done.barrier"), lsp::FileChangeType::CHANGED);
    let after = lsp::Url::from_file_path(uri_path.join("after.watched")).unwrap();
    let barrier = lsp::Url::from_file_path(uri_path.join("done.barrier")).unwrap();
    // FIFO delivery to the same server makes the barrier a deterministic absence check.
    let alpha_log = logged(&f.logs[0], |m| {
        m["params"]["changes"][0]["uri"] == barrier.as_str()
    })
    .await?;
    assert!(!alpha_log
        .iter()
        .any(|m| m["params"]["changes"][0]["uri"] == after.as_str()));
    logged(&f.logs[1], |m| {
        m["params"]["changes"][0]["uri"] == after.as_str()
    })
    .await?;
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_adapter_preserves_typed_results_errors_and_null_acknowledgements(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["alpha"])?;
    f.initialize().await?;
    let server = f.server("alpha");
    let folders = json!(
        &*f.app
            .editor
            .language_server_by_id(server)
            .unwrap()
            .workspace_folders()
            .await
    );
    let invalid_edit = json!({"edit": {"documentChanges": [edit(&f, 100, "stale!!!")]}});
    let cases = [
        (
            "workspace/configuration",
            json!({"items": [{"section": "lifecycle"}, {"section": "missing"}]}),
            json!(["alpha", null]),
        ),
        ("workspace/workspaceFolders", Value::Null, folders),
        (
            "workspace/applyEdit",
            invalid_edit,
            json!({"applied": false, "failureReason": "document has changed", "failedChange": 0}),
        ),
        (
            "client/registerCapability",
            json!({"registrations": [{"id": "ignored", "method": "textDocument/hover"}]}),
            Value::Null,
        ),
        (
            "client/unregisterCapability",
            json!({"unregisterations": [{"id": "ignored", "method": "textDocument/hover"}]}),
            Value::Null,
        ),
        ("workspace/diagnostic/refresh", Value::Null, Value::Null),
    ];
    for (index, (method, params, expected)) in cases.into_iter().enumerate() {
        let id = format!("workspace-{index}");
        let call = serde_json::from_value(
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        )?;
        f.app.handle_language_server_message(call, server).await;
        let messages = logged(&f.logs[0], |m| m["id"] == id).await?;
        let reply = messages.iter().find(|m| m["id"] == id).unwrap();
        assert_eq!(reply.get("result"), Some(&expected), "{method}: {reply}");
        assert!(reply.get("error").is_none());
    }
    let call = serde_json::from_value(
        json!({"jsonrpc": "2.0", "id": "malformed", "method": "workspace/applyEdit", "params": {}}),
    )?;
    f.app.handle_language_server_message(call, server).await;
    let messages = logged(&f.logs[0], |m| m["id"] == "malformed").await?;
    let reply = messages.iter().find(|m| m["id"] == "malformed").unwrap();
    assert_eq!(reply["error"]["code"], -32700);
    assert!(reply.get("result").is_none());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn native_and_batched_file_changes_only_reach_the_owning_editors_servers(
) -> anyhow::Result<()> {
    use super::helpers::run_event_loop_until_idle;
    use view::file_watcher::{CanonicalPathBuf, Event, EventType, Events};

    let first_dir = tempfile::tempdir()?;
    let second_dir = tempfile::tempdir()?;
    let watched_dir = tempfile::tempdir()?;
    let root = watched_dir.path().canonicalize()?;
    let mut first = Fixture::new(first_dir.path(), &["alpha"])?;
    let mut second = Fixture::new(second_dir.path(), &["alpha"])?;
    for fixture in [&mut first, &mut second] {
        fixture.initialize().await?;
        fixture.app.editor.register_language_server_capabilities(
            fixture.server("alpha"),
            serde_json::from_value(json!({"registrations": [
                registration("first", json!([{"globPattern": "**/*.watched"}])),
                registration("overlapping", json!([{"globPattern": "**/*.watched"}])),
                registration("barrier", json!([{"globPattern": "**/*.barrier"}]))
            ]}))?,
        );
    }
    // Only the first editor watches this root. Both servers are interested in it.
    first.app.editor.file_watcher.reload(&Config::default());
    first.app.editor.file_watcher.add_root(&root);
    let native = root.join("native.watched");
    tokio::time::timeout(Duration::from_secs(10), async {
        while !first.app.editor.file_watcher.is_watching(&native) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    std::fs::write(&native, "native change\n")?;
    let native_uri = lsp::Url::from_file_path(&native).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(&mut first.app).await;
            let contents = std::fs::read_to_string(&first.logs[0])?;
            if contents
                .split_inclusive('\n')
                .filter(|line| line.ends_with('\n'))
                .map(serde_json::from_str::<Value>)
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .any(|message| {
                    message["params"]["changes"]
                        .as_array()
                        .is_some_and(|changes| {
                            changes
                                .iter()
                                .any(|change| change["uri"] == native_uri.as_str())
                        })
                })
            {
                return anyhow::Ok(());
            }
        }
    })
    .await??;
    first.app.editor.file_watcher.reload(&Config {
        enable: false,
        ..Default::default()
    });
    first.app.editor.reset_idle_timer();
    run_event_loop_until_idle(&mut first.app).await;
    second.app.editor.reset_idle_timer();
    run_event_loop_until_idle(&mut second.app).await;

    // The second editor gets one explicit batch. Preserve kinds and ordering,
    // deduplicate overlapping registrations, and omit native temporary-file events.
    let batch = root.join("batch.watched");
    let batch_uri = lsp::Url::from_file_path(&batch).unwrap();
    second.app.editor.handle_file_events(&Events::from(
        [
            EventType::Create,
            EventType::Modified,
            EventType::Modified,
            EventType::Tempfile,
            EventType::Delete,
        ]
        .map(|ty| Event {
            path: CanonicalPathBuf::assert_canonicalized(&batch),
            ty,
        })
        .to_vec(),
    ));
    let barrier = root.join("done.barrier");
    let barrier_uri = lsp::Url::from_file_path(&barrier).unwrap();
    for fixture in [&mut first, &mut second] {
        fixture
            .app
            .editor
            .language_servers
            .file_event_handler
            .file_changed(barrier.clone(), lsp::FileChangeType::CHANGED);
    }
    let mut changes_by_editor = Vec::new();
    for fixture in [&first, &second] {
        let messages = logged(&fixture.logs[0], |message| {
            message["params"]["changes"][0]["uri"] == barrier_uri.as_str()
        })
        .await?;
        let changes: Vec<Value> = messages
            .iter()
            .filter(|message| message["method"] == "workspace/didChangeWatchedFiles")
            .flat_map(|message| {
                message["params"]["changes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .cloned()
            })
            .collect();
        changes_by_editor.push(changes);
    }
    assert!(changes_by_editor[0]
        .iter()
        .any(|change| change["uri"] == native_uri.as_str()));
    assert!(!changes_by_editor[0]
        .iter()
        .any(|change| change["uri"] == batch_uri.as_str()));
    assert!(!changes_by_editor[1]
        .iter()
        .any(|change| change["uri"] == native_uri.as_str()));
    let batch_changes: Vec<_> = changes_by_editor[1]
        .iter()
        .filter(|change| change["uri"] == batch_uri.as_str())
        .map(|change| change["type"].clone())
        .collect();
    assert_eq!(batch_changes, [json!(1), json!(2), json!(3)]);
    assert!(first.app.close().await.is_empty());
    assert!(second.app.close().await.is_empty());
    Ok(())
}

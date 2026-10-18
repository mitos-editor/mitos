use std::time::Duration;

use anyhow::Context as _;
use view::{
    current, current_ref,
    editor::{Action, FileOperation},
};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};

async fn wait_for_request(logs: &[std::path::PathBuf]) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            for log in logs {
                if lsp::read_log(log)?
                    .iter()
                    .any(|message| message["method"] == "workspace/willRenameFiles")
                {
                    return anyhow::Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread")]
async fn file_preflight_keeps_the_editor_available_and_rejects_changed_targets(
) -> anyhow::Result<()> {
    for change in ["none", "document", "path", "closed", "invalid"] {
        let dir = tempfile::tempdir()?;
        let initialize_gate = Gate::new(dir.path().join("initialize-ready"));
        let preflight_gate = Gate::new(dir.path().join("preflight-ready"));
        let names: &[&str] = if change == "none" {
            &["alpha", "beta"]
        } else {
            &["alpha"]
        };
        let servers: String = names
            .iter()
            .map(|name| {
                ServerConfig::feature(name, "--file-operations", dir.path())
                    .initialize_gate(&initialize_gate)
                    .arg("--preflight-gate")
                    .arg(preflight_gate.path())
                    .toml()
            })
            .collect();
        let mut config = test_config();
        config.editor.lsp.enable = true;
        let (sender, mut callbacks) = super::helpers::callbacks::deferred(|_, callback| callback);
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(lsp::syntax_loader(
                "files-test",
                "files-test",
                names,
                &servers,
            ))
            .with_handler_setup(move |handlers, config| {
                *handlers =
                    view::handlers::Handlers::new(config, sender(handlers.redraw().clone()));
            })
            .build()?;
        let source = dir.path().join("source.files-test");
        let target = dir.path().join("target.files-test");
        std::fs::write(
            &source,
            if change == "invalid" {
                "reject\n"
            } else {
                "original\n"
            },
        )?;
        app.editor.open(&source, Action::Replace)?;
        initialize_gate.release()?;
        lsp::initialize(&mut app, names.len()).await?;
        app.editor.request_file_operation(FileOperation::Move {
            from: source.clone(),
            to: target.clone(),
        })?;
        let logs: Vec<_> = names
            .iter()
            .map(|name| lsp::log_path(dir.path(), name))
            .collect();
        wait_for_request(&logs).await?;
        assert!(source.exists());
        assert!(!target.exists());
        assert!(app
            .editor
            .request_file_operation(FileOperation::Delete {
                path: source.clone(),
                recursive: false
            })
            .is_err());
        match change {
            "document" => {
                let (view, doc) = current!(app.editor);
                let edit = editor_core::Transaction::insert(
                    doc.text(),
                    doc.selection(view.id),
                    "typed".into(),
                );
                assert!(view::editing::apply(doc, &edit, view.id));
            }
            "path" => std::fs::write(&target, "external\n")?,
            "closed" => {
                let doc = current_ref!(app.editor).1.id();
                app.editor.new_file(Action::Replace);
                assert!(app.editor.close_document(doc, true).is_ok());
            }
            _ => {}
        }
        preflight_gate.release()?;
        // Execute the actual editor callback, including its validation after queueing.
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let callback = callbacks
                    .recv()
                    .await
                    .context("missing preflight callback")?;
                callback(&mut app.editor);
                if !source.exists()
                    || app.editor.status_msg.as_ref().is_some_and(|(message, _)| {
                        message.contains("Could not complete file operation")
                    })
                {
                    return anyhow::Ok(());
                }
            }
        })
        .await??;
        if change == "none" {
            assert!(!source.exists());
            assert_eq!(
                current_ref!(app.editor).1.path(),
                Some(stdx::path::canonicalize(&target).as_path())
            );
            assert_eq!(
                current_ref!(app.editor).1.text(),
                "prepared prepared original\n"
            );
            // Resource edits supplied by a server execute in protocol order without
            // sending another willCreate/willRename/willDelete preflight request.
            let created = dir.path().join("created.files-test");
            let renamed = dir.path().join("renamed.files-test");
            let edit = serde_json::from_value(serde_json::json!({"documentChanges":[
                {"kind":"create", "uri":lsp_client::Url::from_file_path(&created).unwrap()},
                {"kind":"rename", "oldUri":lsp_client::Url::from_file_path(&created).unwrap(), "newUri":lsp_client::Url::from_file_path(&renamed).unwrap()},
                {"kind":"delete", "uri":lsp_client::Url::from_file_path(&renamed).unwrap()}
            ]}))?;
            app.editor
                .apply_workspace_edit(lsp_client::OffsetEncoding::Utf8, &edit)?;
            assert!(!created.exists() && !renamed.exists());
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let log = lsp::read_log(&lsp::log_path(dir.path(), "alpha"))?;
                    if log
                        .iter()
                        .any(|message| message["method"] == "workspace/didDeleteFiles")
                    {
                        assert_eq!(
                            log.iter()
                                .filter(|message| message["method"]
                                    .as_str()
                                    .is_some_and(|method| method.starts_with("workspace/will")))
                                .count(),
                            1
                        );
                        return anyhow::Ok(());
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await??;
        } else {
            assert!(source.exists());
            assert!(app
                .editor
                .status_msg
                .as_ref()
                .is_some_and(
                    |(message, _)| message.contains("changed") || message.contains("stopped")
                ));
            if change == "path" {
                assert_eq!(std::fs::read_to_string(&target)?, "external\n");
            }
        }
    }
    Ok(())
}

use std::{path::PathBuf, time::Duration};

use loader::workspace_trust::{Config, TrustStatus};
use view::{
    current_ref,
    editor::{Action, ConfigEvent, EditorEvent},
    events::DocumentDidOpen,
    handlers::workspace_trust::{
        dismiss_request, next_request, resolve_request, TrustDecision, WorkspaceTrustHandler,
    },
};

use super::Fixture;

fn restrict(f: &mut Fixture) -> anyhow::Result<PathBuf> {
    let workspace = current_ref!(f.editor).1.workspace_root().to_path_buf();
    std::fs::create_dir_all(workspace.join(".mitos"))?;
    std::fs::write(workspace.join(".mitos/config.toml"), "[editor]\n")?;
    f.editor.workspace_trust.set_config(Config::default());
    opened(f);
    Ok(workspace)
}

fn opened(f: &mut Fixture) {
    let doc = current_ref!(f.editor).1.id();
    event::dispatch(DocumentDidOpen {
        editor: &mut f.editor,
        doc,
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn prompts_are_delivered_once_per_editor_and_survive_config_reload() -> anyhow::Result<()> {
    let mut first = Fixture::new("")?;
    let mut second = Fixture::new("")?;
    let path = current_ref!(first.editor).1.path().unwrap().to_path_buf();
    second.editor.open(&path, Action::Replace)?;
    let workspace = restrict(&mut first)?;
    restrict(&mut second)?;
    opened(&mut first);
    let request = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let EditorEvent::WorkspaceTrust(request) = first.editor.wait_event().await {
                break request;
            }
        }
    })
    .await?;
    assert_eq!(request.workspace(), workspace);
    assert!(next_request(&mut first.editor).is_none());
    assert!(next_request(&mut second.editor).is_some());
    // Even matching paths and document IDs do not allow cross-editor responses.
    assert!(!resolve_request(
        &mut second.editor,
        &request,
        TrustDecision::Trust
    )?);
    assert!(request.is_current(&first.editor));
    dismiss_request(&mut first.editor, &request);
    assert!(!resolve_request(
        &mut first.editor,
        &request,
        TrustDecision::Trust
    )?);
    first.editor.workspace_trust.set_config(Config::default());
    opened(&mut first);
    assert!(next_request(&mut first.editor).is_none());
    assert_eq!(
        first.editor.workspace_trust.status(&workspace),
        TrustStatus::Untrusted
    );
    assert!(!matches!(
        first.editor.config_events.1.try_recv(),
        Ok(ConfigEvent::Refresh)
    ));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_workspace_prompts_are_rejected_before_delivery_and_resolution() -> anyhow::Result<()>
{
    for before_delivery in [true, false] {
        for change in ["config", "close", "rename", "policy", "handler", "implicit"] {
            let mut f = Fixture::new("")?;
            let workspace = restrict(&mut f)?;
            let request = (!before_delivery).then(|| next_request(&mut f.editor).unwrap());
            match change {
                "config" => std::fs::write(
                    workspace.join(".mitos/config.toml"),
                    "[editor]\nmouse = false\n",
                )?,
                "close" => f.close_current()?,
                "rename" => {
                    let other = f.dir.path().join("other/.mitos");
                    std::fs::create_dir_all(&other)?;
                    let doc = current_ref!(f.editor).1.id();
                    f.editor
                        .documents
                        .get_mut(&doc)
                        .unwrap()
                        .set_path(Some(&other.parent().unwrap().join("renamed.words")));
                }
                "policy" => f.editor.workspace_trust.set_config(Config {
                    prompt: false,
                    ..Config::default()
                }),
                "handler" => f.editor.handlers.workspace_trust = WorkspaceTrustHandler::default(),
                "implicit" => {
                    f.editor.workspace_trust =
                        loader::workspace_trust::WorkspaceTrust::fully_trusted()
                }
                _ => unreachable!(),
            }
            if let Some(request) = request {
                assert!(
                    !resolve_request(&mut f.editor, &request, TrustDecision::Trust)?,
                    "{change}"
                );
            } else {
                assert!(next_request(&mut f.editor).is_none(), "{change}");
            }
            assert_ne!(
                f.editor.workspace_trust.status(&workspace),
                TrustStatus::Trusted,
                "{change}"
            );
            assert!(
                !matches!(
                    f.editor.config_events.1.try_recv(),
                    Ok(ConfigEvent::Refresh)
                ),
                "{change}"
            );
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_prompts_and_unrestricted_workspaces_do_not_consume_prompt_history(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let workspace = current_ref!(f.editor).1.workspace_root().to_path_buf();
    f.editor.workspace_trust.set_config(Config::default());
    opened(&mut f);
    assert!(next_request(&mut f.editor).is_none());
    std::fs::create_dir_all(workspace.join(".mitos"))?;
    std::fs::write(workspace.join(".mitos/config.toml"), "")?;
    f.editor.workspace_trust.set_config(Config {
        prompt: false,
        ..Config::default()
    });
    opened(&mut f);
    assert!(next_request(&mut f.editor).is_none());
    f.editor.workspace_trust.set_config(Config::default());
    opened(&mut f);
    assert!(next_request(&mut f.editor).is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_restart_preserves_selection_errors_and_missing_executable_behavior(
) -> anyhow::Result<()> {
    let mut f = Fixture::with_languages(
        "",
        r#"
        [language-server.absent]
        command = "mitos-nonexistent-workspace-trust-test-server"
        [[language]]
        name = "restart-test"
        scope = "source.restart-test"
        file-types = ["words"]
        language-servers = ["absent"]
    "#,
    )?;
    let document = current_ref!(f.editor).1.id();
    f.editor.restart_language_servers(document, &[])?;
    assert!(f
        .editor
        .restart_language_servers(document, &["absent"])
        .unwrap_err()
        .to_string()
        .starts_with("Error restarting language servers:"));
    assert_eq!(
        f.editor
            .restart_language_servers(document, &["one", "two"])
            .unwrap_err()
            .to_string(),
        "Unknown language servers: one, two"
    );
    let mut f = Fixture::new("")?;
    let document = current_ref!(f.editor).1.id();
    assert_eq!(
        f.editor
            .restart_language_servers(document, &[])
            .unwrap_err()
            .to_string(),
        "LSP not defined for the current document"
    );
    Ok(())
}

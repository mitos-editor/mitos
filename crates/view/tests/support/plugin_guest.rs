//! Small real guests that validate owned event requests at the WASM boundary.
use std::path::Path;

use plugin_api::{Action, Response};
use plugins::PluginConfig;

#[derive(Default, serde::Serialize)]
pub(crate) struct Route<'a> {
    pub event: &'a str,
    pub response: Response,
    pub expected: Vec<String>,
    pub filter: Option<String>,
}

pub(crate) fn status(message: &str) -> Response {
    Response {
        actions: vec![Action::Status {
            message: message.into(),
        }],
        error: None,
    }
}

/// Optional ordering guard requires the named event before shutdown. Unmatched
/// events return no effects; checks are real guest code rather than host mocks.
pub(crate) fn observing(
    dir: &Path,
    subscriptions: &[&str],
    routes: &[Route<'_>],
    before_shutdown: Option<&str>,
) -> anyhow::Result<PluginConfig> {
    std::fs::write(
        dir.join("plugin.component.wasm"),
        include_bytes!("../../../plugins/tests/fixtures/router-guest.component.wasm"),
    )?;
    std::fs::write(dir.join("plugin.toml"), format!("abi-version = {}\nmodule = 'plugin.component.wasm'\ncapabilities = ['ui', 'editor-read', 'editor-edit', 'editor-selection', 'editor-navigate', 'workspace-read']\nevents = {subscriptions:?}\n[commands.run]\ndoc = 'Observed guest'\n", plugin_api::ABI_VERSION))?;
    Ok(PluginConfig {
        path: dir.join("plugin.toml"),
        enabled: true,
        config: serde_json::json!({"routes":routes,"before_shutdown":before_shutdown}),
        permissions: permissions(dir),
        ..PluginConfig::default()
    })
}

/// Fixture authority is explicit so production defaults stay restricted.
pub(crate) fn permissions(root: &Path) -> plugin_api::Permissions {
    use plugin_api::Capability;
    plugin_api::Permissions {
        capabilities: [
            Capability::Ui,
            Capability::EditorRead,
            Capability::EditorEdit,
            Capability::EditorSelection,
            Capability::EditorNavigate,
            Capability::WorkspaceRead,
        ]
        .into(),
        read_roots: vec![root.to_owned()],
        ..plugin_api::Permissions::default()
    }
}

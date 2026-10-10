//! Small real guests that validate owned event requests at the WASM boundary.
use std::path::Path;

use plugin_api::{Action, Response};
use plugins::PluginConfig;

/// Each proof owns a production engine and compiles the real SDK component.
/// Run one compilation-heavy proof at a time so cold debug compilation on a
/// small runner stays within unchanged production deadlines. Multiple editors
/// inside a proof still exercise independent concurrent instances normally.
/// Acquire once per test (not per Fixture: some cases create multiple editors).
pub(crate) async fn compilation_permit() -> tokio::sync::OwnedSemaphorePermit {
    static PERMITS: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    PERMITS
        .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(1)))
        .clone()
        .acquire_owned()
        .await
        .expect("test compilation budget closed")
}

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
    let mut routes = serde_json::to_value(routes)?;
    for route in routes.as_array_mut().unwrap() {
        let mut metadata = Vec::new();
        let mut reads = Vec::new();
        // UiResult owns the literal text submitted to a native prompt. Its
        // `text` is result metadata, unlike the legacy document-text checks.
        let document_text = route["event"] != "ui-result";
        for fragment in route["expected"].as_array().unwrap() {
            let fragment = fragment.as_str().unwrap();
            if let Some(value) = fragment.strip_prefix("\"text\":").filter(|_| document_text) {
                let text: String = serde_json::from_str(value)?;
                reads.push(serde_json::json!({
                    "start":0, "end":text.chars().count(), "expected":text,
                }));
            } else {
                metadata.push(fragment.to_owned());
            }
        }
        // Metadata ABI 3 deliberately omits document text. The real guest uses
        // this invocation's document ID and version for bounded region reads;
        // its config is removed before metadata matching, so expected fragments
        // cannot match themselves. Stale saved versions must return typed stale
        // errors in explicit read routes rather than silently refreshing here.
        route["expected"] = serde_json::to_value(metadata)?;
        route["reads"] = serde_json::to_value(reads)?;
    }
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

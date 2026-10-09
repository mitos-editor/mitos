use std::{fs, sync::Arc};

use plugin_api::{Action, HostFuture, HostServices, ReadRequest};
use serde_json::json;
use tempfile::TempDir;

use super::*;

fn wasm() -> Vec<u8> {
    wit_component::ComponentEncoder::default()
        .module(include_bytes!("../tests/fixtures/component-guest.wasm"))
        .unwrap()
        .validate(true)
        .encode()
        .unwrap()
}
fn manifest() -> String {
    format!(
        "abi-version = {ABI_VERSION}\nmodule = 'plugin.wasm'\ncapabilities = ['ui', 'editor-read']\nevents = ['document-opened']\n[commands.status]\ndoc = 'Report plugin state'\n[commands.loop]\ndoc = 'Fault fixture'\n"
    )
}
fn fixture(manifest: &str, bytes: &[u8]) -> (TempDir, PluginConfig) {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("plugin.toml"), manifest).unwrap();
    fs::write(directory.path().join("plugin.wasm"), bytes).unwrap();
    let config = PluginConfig {
        path: directory.path().join("plugin.toml"),
        config: json!({"prefix": "example"}),
        permissions: Permissions {
            capabilities: [Capability::Ui, Capability::EditorRead].into(),
            ..Permissions::default()
        },
        ..PluginConfig::default()
    };
    (directory, config)
}
async fn load(config: PluginConfig) -> PluginManager {
    let manager = PluginManager::default();
    manager
        .prepare(
            BTreeMap::from([("example".into(), config)]),
            PathBuf::from("."),
            1,
        )
        .unwrap()
        .await
        .unwrap()
        .activate()
        .unwrap()
}
struct Services;
impl HostServices for Services {
    fn read_document(&self, _request: ReadRequest) -> HostFuture<String> {
        Box::pin(async {
            Err(ServiceError::new(
                ErrorCode::UnsupportedInterface,
                "unexpected read",
            ))
        })
    }
}
fn context(generation: u64) -> EditorContext {
    EditorContext {
        generation,
        mode: "normal".into(),
        ..EditorContext::default()
    }
}
async fn command(manager: &PluginManager) -> CompletedResponse {
    manager
        .call_command(
            "example.status",
            vec![],
            context(manager.generation()),
            Arc::new(Services),
        )
        .unwrap()
        .unwrap()
        .await
        .unwrap()
}

#[tokio::test]
async fn commands_are_documented_and_real_guest_state_persists() {
    let (_directory, config) = fixture(&manifest(), &wasm());
    let manager = load(config).await;
    assert_eq!(manager.available_commands().len(), 2);
    assert_eq!(
        manager.get_doc_for_identifier("example.status").as_deref(),
        Some("Report plugin state")
    );
    assert!(manager
        .call_command("native-command", vec![], context(1), Arc::new(Services))
        .unwrap()
        .is_none());
    for expected in ["call 1", "call 2"] {
        let response = command(&manager).await;
        assert!(matches!(&response.actions[0], Action::Status { message } if message == expected));
    }
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn configuration_is_bounded_and_prepared_once_off_editor_path() {
    let declared = format!("{}[commands.config]\ndoc = 'Inspect config'\n", manifest());
    let (_directory, mut config) = fixture(&declared, &wasm());
    let manager = load(config.clone()).await;
    let response = manager
        .call_command("example.config", vec![], context(1), Arc::new(Services))
        .unwrap()
        .unwrap()
        .await
        .unwrap();
    assert!(
        matches!(&response.actions[0], Action::Status { message } if message == r#"{"prefix":"example"}"#)
    );
    config.config = json!({"large": "x".repeat(64 * 1024)});
    assert!(manager
        .prepare(
            BTreeMap::from([("example".into(), config)]),
            PathBuf::from("."),
            2
        )
        .unwrap()
        .await
        .is_err());
    assert!(manager.subscribes(Event::Init));
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn digest_or_component_failure_preserves_old_generation_and_reuses_pool() {
    let (_directory, config) = fixture(&manifest(), &wasm());
    let manager = load(config.clone()).await;
    drop(command(&manager).await);
    let mut pinned = config.clone();
    pinned.sha256 = Some("0".repeat(64));
    let failure = manager
        .prepare(
            BTreeMap::from([("example".into(), pinned)]),
            PathBuf::from("."),
            2,
        )
        .unwrap()
        .await
        .err()
        .unwrap();
    assert!(failure.message.contains("SHA-256"));
    let response = command(&manager).await;
    assert!(matches!(&response.actions[0], Action::Status { message } if message == "call 2"));
    let prepared = manager
        .prepare(
            BTreeMap::from([("example".into(), config)]),
            PathBuf::from("."),
            2,
        )
        .unwrap()
        .await
        .unwrap();
    assert!(manager
        .pool
        .as_ref()
        .unwrap()
        .same_executor(prepared.pool.as_ref().unwrap()));
    let replacement = prepared.activate().unwrap();
    assert_eq!(replacement.generation(), 2);
    drop(command(&replacement).await);
    manager.shutdown().await.unwrap();
    replacement.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_package_rejects_the_whole_replacement_without_activating_partial_set() {
    let (_directory, config) = fixture(&manifest(), &wasm());
    let (invalid_directory, invalid) = fixture(&manifest(), b"not a component");
    let manager = PluginManager::default();
    let failure = manager
        .prepare(
            BTreeMap::from([("good".into(), config), ("bad".into(), invalid)]),
            PathBuf::from("."),
            1,
        )
        .unwrap()
        .await
        .err()
        .unwrap();
    assert!(failure.message.contains("bad"));
    assert!(manager.available_commands().is_empty());
    assert!(manager.pool.is_none());
    fs::write(
        invalid_directory.path().join("plugin.wasm"),
        wat::parse_str("(module)").unwrap(),
    )
    .unwrap();
    let config = PluginConfig {
        path: invalid_directory.path().join("plugin.toml"),
        ..PluginConfig::default()
    };
    assert!(manager
        .prepare(
            BTreeMap::from([("old-core-abi".into(), config)]),
            PathBuf::from("."),
            1
        )
        .unwrap()
        .await
        .is_err());
}

#[tokio::test]
async fn observation_requires_both_grants_and_declaration_and_revocation_is_immediate() {
    let (_directory, mut config) = fixture(&manifest(), &wasm());
    config.permissions = Permissions::default();
    let manager = load(config).await;
    assert!(!manager.subscribes(Event::DocumentOpened));
    assert!(manager.subscribes(Event::Init));
    assert!(manager.subscribes(Event::UiResult));
    manager.policy("example").unwrap().revoke();
    assert!(!manager.subscribes(Event::Init));
    assert!(manager.available_commands().is_empty());
    assert_eq!(
        manager
            .call_command("example.status", vec![], context(1), Arc::new(Services))
            .err()
            .unwrap()
            .code,
        ErrorCode::Cancelled
    );
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn dropping_manager_revokes_grants_and_callbacks_cannot_resurrect_actor() {
    let (_directory, config) = fixture(&manifest(), &wasm());
    let manager = load(config).await;
    let policy = manager.policy("example").unwrap();
    let actor = manager.plugins["example"].actor.as_ref().unwrap().clone();
    drop(manager);
    assert!(!actor.is_active());
    assert_eq!(
        policy.require(Capability::Ui).unwrap_err().code,
        ErrorCode::Cancelled
    );
    actor.shutdown().await.unwrap();
}

#[tokio::test]
async fn disabled_and_declarative_packages_need_no_executor_and_relative_paths_work() {
    let (_directory, mut config) = fixture(&manifest(), &wasm());
    let base = config.path.parent().unwrap().to_owned();
    config.path = "plugin.toml".into();
    let manager = PluginManager::default();
    let configured = manager
        .prepare(
            BTreeMap::from([("example".into(), config.clone())]),
            base.clone(),
            1,
        )
        .unwrap()
        .await
        .unwrap()
        .activate()
        .unwrap();
    assert_eq!(configured.available_commands().len(), 2);
    configured.shutdown().await.unwrap();
    config.enabled = false;
    config.path = "missing.toml".into();
    let disabled = manager
        .prepare(
            BTreeMap::from([("example".into(), config)]),
            base.clone(),
            2,
        )
        .unwrap()
        .await
        .unwrap()
        .activate()
        .unwrap();
    assert!(disabled.pool.is_none());
    assert!(disabled.available_commands().is_empty());
    fs::write(
        base.join("plugin.toml"),
        format!("abi-version = {ABI_VERSION}\n"),
    )
    .unwrap();
    let declarative = manager
        .prepare(
            BTreeMap::from([(
                "example".into(),
                PluginConfig {
                    path: "plugin.toml".into(),
                    ..PluginConfig::default()
                },
            )]),
            base,
            3,
        )
        .unwrap()
        .await
        .unwrap()
        .activate()
        .unwrap();
    assert!(declarative.pool.is_none());
    assert!(!declarative.subscribes(Event::Init));
    let defaults: PluginConfig = toml::from_str("path = 'plugin.toml'").unwrap();
    assert!(defaults.enabled);
    assert_eq!(defaults.config, Value::Null);
}

#[tokio::test]
async fn static_argument_metadata_rejects_wrong_counts_before_guest_admission() {
    let declared = manifest().replacen(
        "doc = 'Report plugin state'",
        "doc = 'Report plugin state'\narguments = { min = 1, max = 1, completions = [['two words', 'μ']] }",
        1,
    );
    let (_directory, config) = fixture(&declared, &wasm());
    let manager = load(config).await;
    let metadata = manager
        .get_arguments_for_identifier("example.status")
        .unwrap();
    assert_eq!((metadata.min, metadata.max), (1, 1));
    assert_eq!(metadata.completions, vec![vec!["two words", "μ"]]);
    for args in [vec![], vec!["one".into(), "two".into()]] {
        assert_eq!(
            manager
                .call_command("example.status", args, context(1), Arc::new(Services))
                .err()
                .unwrap()
                .code,
            ErrorCode::InvalidRequest
        );
    }
    let response = manager
        .call_command(
            "example.status",
            vec!["μ".into()],
            context(1),
            Arc::new(Services),
        )
        .unwrap()
        .unwrap()
        .await
        .unwrap();
    assert!(matches!(&response.actions[0], Action::Status { message } if message == "call 1"));
    manager.shutdown().await.unwrap();
}

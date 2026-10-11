//! Drive plugins through the headless editor fixture.
use super::Fixture;
use plugins::PluginConfig;
use std::{collections::BTreeMap, path::Path};

/// Settle real guest work and execute callbacks on the editor thread.
pub(crate) async fn drain(fixture: &mut Fixture) {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            fixture.editor.poll_plugin_events();
            while let Ok(callback) = fixture.callbacks.try_recv() {
                callback(&mut fixture.editor);
            }
            if !fixture.editor.has_pending_plugin_work() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("plugin work did not settle");
}

pub(crate) async fn load(
    fixture: &mut Fixture,
    base: &Path,
    packages: &BTreeMap<String, PluginConfig>,
) {
    let errors = fixture.editor.error_revision();
    assert!(fixture.editor.reload_plugins(packages, base));
    drain(fixture).await;
    assert_eq!(
        fixture.editor.error_revision(),
        errors,
        "status={:?}; diagnostics={:?}",
        fixture.editor.get_status(),
        fixture.editor.plugin_diagnostics()
    );
    for (name, package) in packages {
        if package.enabled {
            assert!(
                fixture
                    .editor
                    .plugin_command(&format!("{name}.run"))
                    .is_some(),
                "package '{name}' did not activate: status={:?}; diagnostics={:?}",
                fixture.editor.get_status(),
                fixture.editor.plugin_diagnostics()
            );
        }
    }
}

pub(crate) async fn execute(
    fixture: &mut Fixture,
    name: &str,
    args: Vec<String>,
) -> anyhow::Result<bool> {
    let errors = fixture.editor.error_revision();
    let accepted = fixture.editor.execute_plugin_command(name, args)?;
    drain(fixture).await;
    anyhow::ensure!(
        fixture.editor.error_revision() == errors,
        "status={:?}; diagnostics={:?}",
        fixture.editor.get_status(),
        fixture.editor.plugin_diagnostics()
    );
    Ok(accepted)
}

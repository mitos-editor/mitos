pub mod migration;

use std::str::from_utf8;

use workspace::trust::{TrustQuery, WorkspaceTrust};

/// Default built-in languages.toml.
pub fn default_lang_config() -> toml::Value {
    let default_config = include_bytes!("../../../languages.toml");
    toml::from_str(from_utf8(default_config).unwrap())
        .expect("Could not parse built-in languages.toml to valid toml")
}

/// User configured languages.toml file, merged with the default config.
///
/// Workspace-local `.mitos/languages.toml` is merged in only when the current
/// workspace is trusted for [`TrustQuery::LocalConfig`].
pub fn user_lang_config(trust: &WorkspaceTrust) -> Result<toml::Value, toml::de::Error> {
    let global_config = crate::lang_config_file();
    let workspace_config = crate::workspace_lang_config_file();

    let files = if trust.query_current(TrustQuery::LocalConfig).is_trusted() {
        vec![global_config, workspace_config]
    } else {
        vec![global_config]
    };

    let config = files
        .iter()
        .filter_map(|file| {
            std::fs::read_to_string(file)
                .map(|config| toml::from_str(&config))
                .ok()
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .fold(default_lang_config(), |a, b| {
            migration::merge_languages(a, b)
        });

    Ok(config)
}

/// Print a notice if the current workspace has a `.mitos/languages.toml` that we *would* have
/// merged but the workspace-trust gate is keeping us from.
pub fn warn_if_workspace_languages_skipped(trust: &workspace::trust::WorkspaceTrust) {
    let workspace_languages = crate::workspace_lang_config_file();
    if !workspace_languages.exists() {
        return;
    }
    if trust
        .query_current(workspace::trust::TrustQuery::LocalConfig)
        .is_trusted()
    {
        return;
    }
    println!(
        "Note: workspace `{}` was skipped because the workspace is not trusted. Run \
         `:workspace-trust` from an interactive mitos session in this workspace to opt in.",
        workspace_languages.display(),
    );
}

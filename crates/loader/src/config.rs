pub mod migration;

use std::{path::PathBuf, str::from_utf8};
use workspace::{
    trust::{TrustQuery, WorkspaceTrust},
    ResolvedWorkspace,
};

/// Paths and the global policy permitting workspace-local configuration.
/// Construct after loading global trust settings; local contents cannot grant access.
pub struct ConfigSources {
    workspace: ResolvedWorkspace,
    local_allowed: bool,
}

impl ConfigSources {
    pub fn new(workspace: &ResolvedWorkspace, trust: &WorkspaceTrust) -> Self {
        Self {
            workspace: workspace.clone(),
            local_allowed: trust
                .query(workspace.root(), TrustQuery::LocalConfig)
                .is_trusted(),
        }
    }

    pub fn local_file(&self, name: &str) -> Option<PathBuf> {
        self.local_allowed
            .then(|| self.workspace.root().join(".mitos").join(name))
    }

    /// Read global then permitted workspace languages, preserving best-effort I/O.
    /// Diagnostics are returned even if a permitted source fails to parse.
    pub fn languages(&self) -> LanguageSources {
        self.languages_from(crate::lang_config_file())
    }

    fn languages_from(&self, global: PathBuf) -> LanguageSources {
        let local = self.workspace.root().join(".mitos/languages.toml");
        let skipped = (!self.local_allowed && local.exists()).then_some(SkippedConfig(local));
        let value = std::iter::once(global)
            .chain(self.local_file("languages.toml"))
            .filter_map(|file| std::fs::read_to_string(file).ok())
            .try_fold(default_lang_config(), |base, source| {
                toml::from_str(&source).map(|layer| migration::merge_languages(base, layer))
            });
        LanguageSources { value, skipped }
    }
}

pub struct LanguageSources {
    pub value: Result<toml::Value, toml::de::Error>,
    pub skipped: Option<SkippedConfig>,
}

/// Grammar commands use the conservative default policy, never an implicit local grant.
pub fn grammar_sources(workspace: &ResolvedWorkspace) -> LanguageSources {
    let trust = WorkspaceTrust::new(Default::default(), crate::workspace_trust_dir());
    ConfigSources::new(workspace, &trust).languages()
}

/// Default built-in languages.toml.
pub fn default_lang_config() -> toml::Value {
    let default_config = include_bytes!("../../../languages.toml");
    toml::from_str(from_utf8(default_config).unwrap())
        .expect("Could not parse built-in languages.toml to valid toml")
}

/// A workspace source omitted without reading its contents.
pub struct SkippedConfig(PathBuf);

impl std::fmt::Display for SkippedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Note: workspace `{}` was skipped because the workspace is not trusted. Run :workspace-trust from an interactive mitos session in this workspace to opt in.", self.0.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_plan_excludes_local_contents_and_retains_diagnostics_on_parse_errors() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join(".mitos")).unwrap();
        let local = root.join(".mitos/languages.toml");
        std::fs::write(&local, "invalid [toml").unwrap();
        let global = root.join("languages.toml");
        let workspace = ResolvedWorkspace::resolve(root);
        let trust = WorkspaceTrust::new(Default::default(), root.join("trust"));
        let sources = ConfigSources::new(&workspace, &trust);
        assert!(sources.local_file("config.toml").is_none());
        let report = sources.languages_from(global.clone());
        assert!(report.skipped.is_some());
        assert!(report.value.is_ok()); // Invalid untrusted content was never parsed.
        std::fs::write(&global, "also invalid [toml").unwrap();
        let report = sources.languages_from(global.clone());
        assert!(report
            .skipped
            .unwrap()
            .to_string()
            .contains(&local.display().to_string()));
        assert!(report.value.is_err());
        std::fs::remove_file(&global).unwrap();
        trust.trust(workspace.root());
        let report = ConfigSources::new(&workspace, &trust).languages_from(global);
        assert!(report.skipped.is_none());
        assert!(report.value.is_err()); // The same content is rejected when explicitly trusted.
    }

    #[test]
    fn trusted_layers_override_in_order_and_io_failures_remain_best_effort() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join(".mitos")).unwrap();
        let global = root.join("languages.toml");
        std::fs::write(&global, "[language-server.example]\ncommand='global'").unwrap();
        let local = root.join(".mitos/languages.toml");
        std::fs::write(&local, "[language-server.example]\ncommand='local'").unwrap();
        let workspace = ResolvedWorkspace::resolve(root);
        let trust = WorkspaceTrust::fully_trusted(root.join("trust"));
        let sources = ConfigSources::new(&workspace, &trust);
        let value = sources.languages_from(global.clone()).value.unwrap();
        assert_eq!(
            value["language-server"]["example"]["command"].as_str(),
            Some("local")
        );
        std::fs::remove_file(&local).unwrap();
        std::fs::create_dir(&local).unwrap();
        let value = sources.languages_from(global).value.unwrap();
        assert_eq!(
            value["language-server"]["example"]["command"].as_str(),
            Some("global")
        );
    }
}

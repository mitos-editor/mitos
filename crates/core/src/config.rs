use loader::{syntax::Resources, workspace_trust::WorkspaceTrust};

use crate::syntax::{
    config::{Configuration, LanguageConfiguration},
    Loader, LoaderError,
};

/// Language configuration based on built-in languages.toml.
pub fn default_lang_config() -> Configuration {
    loader::config::default_lang_config()
        .try_into()
        .expect("Could not deserialize built-in languages.toml")
}

/// Language configuration loader based on built-in languages.toml.
pub fn default_lang_loader(resources: Resources) -> Loader {
    Loader::new(default_lang_config(), resources)
        .expect("Could not compile loader for default config")
}

#[derive(Debug)]
pub enum LanguageLoaderError {
    DeserializeError(toml::de::Error),
    ConfigError(toml::de::Error, String),
    LoaderError(LoaderError),
}

impl std::fmt::Display for LanguageLoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeserializeError(err) => write!(f, "Failed to parse language config: {err}"),
            Self::ConfigError(err, context) => {
                write!(f, "Failed to parse language config {context}: {err}")
            }
            Self::LoaderError(err) => write!(f, "Failed to compile language config: {err}"),
        }
    }
}

impl std::error::Error for LanguageLoaderError {}

/// Language configuration based on user configured languages.toml.
pub fn user_lang_config(trust: &WorkspaceTrust) -> Result<Configuration, toml::de::Error> {
    loader::config::user_lang_config(trust)?.try_into()
}

/// Language configuration loader based on user configured languages.toml.
pub fn user_lang_loader(
    trust: &WorkspaceTrust,
    resources: Resources,
) -> Result<Loader, LanguageLoaderError> {
    let config_val =
        loader::config::user_lang_config(trust).map_err(LanguageLoaderError::DeserializeError)?;
    let config = config_val.clone().try_into().map_err(|e| {
        if let Some(languages) = config_val.get("language").and_then(|v| v.as_array()) {
            for lang in languages.iter() {
                let res: Result<LanguageConfiguration, _> = lang.clone().try_into();
                if let Err(inner_err) = res {
                    let context = match lang.get("name") {
                        Some(name) => format!("for language {}", name),
                        None => "for unknown language".to_owned(),
                    };
                    return LanguageLoaderError::ConfigError(inner_err, context);
                }
            }
        }
        LanguageLoaderError::ConfigError(e, String::new())
    })?;
    Loader::new(config, resources).map_err(LanguageLoaderError::LoaderError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_layers_preserve_existing_keys_and_partial_overrides() {
        let base: toml::Value = toml::from_str(
            r#"
            [[language]]
            name = "example"
            scope = "source.example"
            file-types = ["example"]
            auto-format = true
            word-completion = { enable = true, trigger-length = 9 }
            language-servers = ["first", "second"]
            [language-server.first]
            command = "first"
            config = { "display-messages" = false, nested = { value = 1 } }
        "#,
        )
        .unwrap();
        let user: toml::Value = toml::from_str(
            r#"
            [[language]]
            name = "example"
            auto-format = false
            word-completion.enable = false
            path-completion = false
            rainbow-brackets = true
            auto-pairs = false
            workspace-lsp-roots = ["packages"]
            language-servers = []
        "#,
        )
        .unwrap();
        let merged = loader::config::migration::merge_languages(base, user);
        assert_eq!(
            merged["language-server"]["first"]["config"]["display-messages"].as_bool(),
            Some(false)
        );
        let config: Configuration = merged.try_into().unwrap();
        let language = &config.language[0];
        assert_eq!(language.auto_format, Some(false));
        assert_eq!(language.word_completion.unwrap().enable, Some(false));
        assert_eq!(
            language
                .word_completion
                .unwrap()
                .trigger_length
                .unwrap()
                .get(),
            9
        );
        assert_eq!(language.path_completion, Some(false));
        assert_eq!(language.rainbow_brackets, Some(true));
        assert!(language.auto_pairs.is_none());
        assert_eq!(
            language.workspace_lsp_roots.as_deref(),
            Some([std::path::PathBuf::from("packages")].as_slice())
        );
        assert!(language.language_servers.is_empty());
    }

    #[test]
    fn language_schema_rejects_new_editor_paths_and_unknown_feature_keys() {
        for setting in ["format.on-save = false", "word-completion.enabled = true"] {
            let invalid = format!("[[language]]\nname = 'example'\nscope = 'source.example'\nfile-types = ['example']\n{setting}");
            assert!(
                toml::from_str::<Configuration>(&invalid).is_err(),
                "{setting}"
            );
        }
        // Every built-in language must still deserialize with its existing keys.
        assert!(!default_lang_config().language.is_empty());
    }
}

#[cfg(test)]
mod language_server_tests {
    use super::*;
    use crate::syntax::config::LanguageServerFeature;
    use std::path::Path;

    fn language(entries: &str) -> LanguageConfiguration {
        toml::from_str(&format!(
            r#"name = "example"
scope = "source.example"
file-types = ["example"]
language-servers = {entries}
"#
        ))
        .unwrap()
    }

    #[test]
    fn alternative_groups_round_trip_and_keep_per_candidate_feature_filters() {
        let config = language(
            r#"[
            "standalone",
            { alternatives = ["preferred", { name = "fallback", except-features = ["format"] }] },
            { name = "linter", only-features = ["diagnostics", "code-action"] },
        ]"#,
        );
        let groups = config.language_server_groups().collect::<Vec<_>>();
        assert_eq!(
            groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
            [1, 2, 1]
        );
        assert_eq!(groups[1][0].name, "preferred");
        assert!(!groups[1][1].has_feature(LanguageServerFeature::Format));
        assert!(!groups[2][0].has_feature(LanguageServerFeature::Hover));
        let roundtrip: LanguageConfiguration =
            toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(
            roundtrip
                .language_server_groups()
                .map(|group| group.len())
                .collect::<Vec<_>>(),
            [1, 2, 1]
        );
        assert!(!roundtrip.language_servers[2].has_feature(LanguageServerFeature::Format));
        let legacy = language(r#"["first", { name = "second", only-features = ["format"] }]"#);
        assert!(legacy
            .language_servers
            .iter()
            .all(|server| server.alternative_group.is_none()));
    }

    #[test]
    fn invalid_or_ambiguous_alternative_groups_are_rejected() {
        for entries in [
            r#"[{ alternatives = [] }]"#,
            r#"[{ alternatives = ["same", "same"] }]"#,
            r#"["same", { alternatives = ["same", "other"] }]"#,
            r#"[{ alternatives = [{ alternatives = ["nested"] }] }]"#,
            r#"[{ alternatives = ["first"], name = "ambiguous" }]"#,
            r#"[{ alternatives = [{ name = "first", unknown = true }] }]"#,
        ] {
            let text = format!("name = 'test'\nscope = 'source.test'\nfile-types = []\nlanguage-servers = {entries}");
            assert!(
                toml::from_str::<LanguageConfiguration>(&text).is_err(),
                "{entries}"
            );
        }
    }

    #[test]
    fn built_in_servers_are_defined_and_specialized_files_keep_their_language() {
        let config = default_lang_config();
        for language in &config.language {
            for server in &language.language_servers {
                assert!(
                    config.language_server.contains_key(&server.name),
                    "{}: {} is undefined",
                    language.language_id,
                    server.name
                );
            }
        }
        let loader = default_lang_loader(Resources::new(Vec::new()));
        for (path, expected) in [
            ("/project/docs/page.mdx", "mdx"),
            ("/project/docs/page.md", "markdown"),
            ("/project/config.yaml", "yaml"),
            ("/project/playbooks/deploy.yml", "ansible"),
            ("/project/roles/web/tasks/main.yaml", "ansible"),
            ("/project/roles/web/tasks/nested/install.yml", "ansible"),
            ("/project/.github/workflows/ci.yml", "github-action"),
        ] {
            let language = loader.language_for_filename(Path::new(path)).unwrap();
            assert_eq!(
                loader.language(language).config().language_id,
                expected,
                "{path}"
            );
        }
    }
}

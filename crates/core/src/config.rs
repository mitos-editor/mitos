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

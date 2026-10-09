//! Bounded declarative package contributions. These records grant no execution
//! authority; grammar identifiers always refer to the host's approved languages.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{ErrorCode, ServiceError};

pub const MAX_ASSET_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SOURCE_BYTES: usize = 128 * 1024;
pub const MAX_THEMES: usize = 16;
pub const MAX_LANGUAGES: usize = 16;
pub const MAX_SNIPPETS: usize = 256;
pub const MAX_SNIPPET_BYTES: usize = 8192;
pub const MAX_ASSET_NAME_BYTES: usize = 128;
pub const MAX_EXTENSIONS: usize = 32;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Contributions {
    pub themes: Vec<ThemeContribution>,
    pub languages: Vec<LanguageContribution>,
    pub snippets: Vec<SnippetContribution>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeContribution {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageContribution {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnippetContribution {
    pub language: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssetOwner {
    pub plugin: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedAssets {
    pub owner: AssetOwner,
    pub themes: Vec<ThemeAsset>,
    pub languages: Vec<LanguageAsset>,
    pub snippets: Vec<SnippetAsset>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeAsset {
    pub name: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueryKind {
    Highlights,
    Injections,
    Locals,
    Indents,
    Textobjects,
    Tags,
    Rainbows,
    Spellcheck,
}

impl QueryKind {
    pub fn filename(&self) -> &'static str {
        match self {
            Self::Highlights => "highlights.scm",
            Self::Injections => "injections.scm",
            Self::Locals => "locals.scm",
            Self::Indents => "indents.scm",
            Self::Textobjects => "textobjects.scm",
            Self::Tags => "tags.scm",
            Self::Rainbows => "rainbows.scm",
            Self::Spellcheck => "spellcheck.scm",
        }
    }
}

/// Only syntax identity and literal file extensions are configurable. Formatter,
/// language-server, debugger, shell and workspace settings are deliberately absent.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct LanguageProfile {
    pub base_language: String,
    pub scope: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Relative paths in the package file; active snapshots replace them with bytes.
    #[serde(default)]
    pub queries: BTreeMap<QueryKind, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageAsset {
    pub name: String,
    pub profile: LanguageProfile,
    pub queries: BTreeMap<QueryKind, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnippetAsset {
    pub language: String,
    pub prefix: String,
    pub body: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnippetFile {
    pub snippets: Vec<SnippetDefinition>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnippetDefinition {
    pub prefix: String,
    pub body: String,
    #[serde(default)]
    pub description: String,
}

pub fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ASSET_NAME_BYTES
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

fn invalid(message: &str) -> ServiceError {
    ServiceError::new(ErrorCode::InvalidRequest, message)
}

impl OwnedAssets {
    pub fn validate(&self) -> Result<(), ServiceError> {
        if !valid_name(&self.owner.plugin) {
            return Err(invalid("invalid asset owner"));
        }
        if self.themes.len() > MAX_THEMES
            || self.languages.len() > MAX_LANGUAGES
            || self.snippets.len() > MAX_SNIPPETS
        {
            return Err(ServiceError::new(
                ErrorCode::ResourceExhausted,
                "too many package contributions",
            ));
        }
        let mut bytes = 0usize;
        let mut names = BTreeSet::new();
        for theme in &self.themes {
            if !valid_name(&theme.name)
                || !names.insert(("theme", &theme.name))
                || theme.source.len() > MAX_SOURCE_BYTES
            {
                return Err(invalid("invalid, duplicate or oversized theme"));
            }
            bytes += theme.source.len();
        }
        for language in &self.languages {
            let profile = &language.profile;
            if !valid_name(&language.name)
                || !names.insert(("language", &language.name))
                || !valid_name(&profile.base_language)
                || profile.scope.is_empty()
                || profile.scope.len() > MAX_ASSET_NAME_BYTES
                || !profile
                    .scope
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
                || profile.extensions.len() > MAX_EXTENSIONS
                || profile.extensions.iter().any(|ext| !valid_name(ext))
                || language
                    .queries
                    .values()
                    .any(|source| source.len() > MAX_SOURCE_BYTES)
            {
                return Err(invalid("invalid declarative language profile"));
            }
            bytes += language.queries.values().map(String::len).sum::<usize>();
        }
        let mut snippets = BTreeSet::new();
        for snippet in &self.snippets {
            if !valid_name(&snippet.language)
                && !snippet
                    .language
                    .split_once('.')
                    .is_some_and(|(owner, name)| valid_name(owner) && valid_name(name))
            {
                return Err(invalid("invalid snippet language"));
            }
            if snippet.prefix.is_empty()
                || snippet.prefix.len() > MAX_ASSET_NAME_BYTES
                || snippet.prefix.chars().any(char::is_control)
                || snippet.body.len() > MAX_SNIPPET_BYTES
                || snippet.description.len() > 1024
                || snippet.description.chars().any(char::is_control)
                || !snippets.insert((&snippet.language, &snippet.prefix))
            {
                return Err(invalid("invalid, duplicate or oversized snippet"));
            }
            bytes += snippet.prefix.len() + snippet.body.len() + snippet.description.len();
        }
        if bytes > MAX_ASSET_BYTES {
            return Err(ServiceError::new(
                ErrorCode::ResourceExhausted,
                "package contributions exceed byte budget",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_reject_executable_configuration_and_traversal() {
        assert!(serde_json::from_str::<LanguageProfile>(
            r#"{"base-language":"rust","scope":"source.rust","formatter":{"command":"sh"}}"#
        )
        .is_err());
        assert!(!valid_name("../rust"));
        assert_eq!(QueryKind::Highlights.filename(), "highlights.scm");
    }
    #[test]
    fn bounded_assets_reject_duplicate_names() {
        let theme = ThemeAsset {
            name: "night".into(),
            source: String::new(),
        };
        let assets = OwnedAssets {
            owner: AssetOwner {
                plugin: "fixture".into(),
                generation: 1,
            },
            themes: vec![theme.clone(), theme],
            languages: vec![],
            snippets: vec![],
        };
        assert!(assets.validate().is_err());
    }
}

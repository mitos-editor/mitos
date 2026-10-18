//! Spelling settings and validated dictionary identifiers.

use std::fmt;

use serde::{Deserialize, Serialize};
use smartstring::SmartString;

/// A spelling dictionary identifier, such as `en_US`.
///
/// This names the dictionary files (`dictionaries/<id>/<id>.{aff,dic}`) under the runtime
/// directories; it is not otherwise interpreted, so it can be any identifier a dictionary is
/// distributed under (`en_US`, `de_DE_frami`, `ca`, ...).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpellingLanguage(SmartString<smartstring::LazyCompact>);

impl SpellingLanguage {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SpellingLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub struct ParseSpellingLanguageError(String);

impl std::str::FromStr for SpellingLanguage {
    type Err = ParseSpellingLanguageError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // The identifier is interpolated into a dictionary file path, so restrict it to a single
        // safe path component: non-empty ASCII alphanumerics plus `_` and `-`.
        if !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            Ok(Self(s.into()))
        } else {
            Err(ParseSpellingLanguageError(s.to_owned()))
        }
    }
}

impl fmt::Display for ParseSpellingLanguageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid spelling language '{}': expected a dictionary name of ASCII letters, digits, '_' or '-'",
            self.0
        )
    }
}

impl std::error::Error for ParseSpellingLanguageError {}

impl serde::Serialize for SpellingLanguage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for SpellingLanguage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let string = <String as serde::Deserialize>::deserialize(deserializer)?;
        string.parse().map_err(serde::de::Error::custom)
    }
}

/// Spell-checking configuration. The same shape is used globally (`[editor.spelling]`) and
/// per-language in `languages.toml`; a language's settings layer over the global ones (see
/// [`SpellingConfig::merged`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct SpellingConfig {
    /// The dictionaries to check a document against; unset inherits (and the global default is
    /// none, i.e. spell checking off). A word is flagged only when every dictionary rejects it.
    pub languages: Option<Vec<SpellingLanguage>>,
    /// Extra accepted words, checked case-insensitively. A language's `words` are *added* to the
    /// global ones rather than replacing them.
    pub words: Vec<String>,
    /// Tokens matching any of these regexes are not checked (e.g. `"^[A-Z0-9_]+$"` to skip
    /// SCREAMING_CASE). A language's regexes are *added* to the global ones.
    pub ignore_regexes: Vec<String>,
    /// Tokens shorter than this are not checked. Unset inherits; the global default is 1 (check
    /// everything).
    pub min_word_length: Option<usize>,
}

impl SpellingConfig {
    /// Layers a language's `spelling` settings over these (global) ones: `languages` and
    /// `min-word-length` replace, while `words` and `ignore-regexes` are unioned.
    pub fn merged(&self, language: Option<&SpellingConfig>) -> SpellingConfig {
        let Some(language) = language else {
            return self.clone();
        };
        SpellingConfig {
            languages: language
                .languages
                .clone()
                .or_else(|| self.languages.clone()),
            words: self.words.iter().chain(&language.words).cloned().collect(),
            ignore_regexes: self
                .ignore_regexes
                .iter()
                .chain(&language.ignore_regexes)
                .cloned()
                .collect(),
            min_word_length: language.min_word_length.or(self.min_word_length),
        }
    }

    /// The dictionaries to check against (empty when spell checking is off).
    pub fn languages(&self) -> &[SpellingLanguage] {
        self.languages.as_deref().unwrap_or_default()
    }

    /// The shortest token length that is checked.
    pub fn min_word_length(&self) -> usize {
        self.min_word_length.unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_settings_override_scalars_and_extend_filters() {
        let global = SpellingConfig {
            languages: Some(vec!["en_US".parse().unwrap()]),
            words: vec!["Mitos".into()],
            ignore_regexes: vec!["^[A-Z_]+$".into()],
            min_word_length: Some(2),
        };
        let local = SpellingConfig {
            languages: Some(Vec::new()),
            words: vec!["Tokio".into()],
            ignore_regexes: vec!["^v[0-9]+$".into()],
            min_word_length: Some(3),
        };
        let merged = global.merged(Some(&local));
        assert!(merged.languages().is_empty());
        assert_eq!(merged.words, ["Mitos", "Tokio"]);
        assert_eq!(merged.ignore_regexes, ["^[A-Z_]+$", "^v[0-9]+$"]);
        assert_eq!(merged.min_word_length(), 3);
        assert_eq!(global.merged(None), global);
    }

    #[test]
    fn dictionary_identifiers_are_safe_case_sensitive_path_components() {
        for valid in ["en_US", "de_DE_frami", "en-GB", "ca"] {
            let language: SpellingLanguage = valid.parse().unwrap();
            assert_eq!(language.as_str(), valid);
        }
        for invalid in [
            "",
            "..",
            "../en_US",
            "de/DE",
            r"de\DE",
            " en_US",
            "en_US.txt",
        ] {
            assert!(invalid.parse::<SpellingLanguage>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn configuration_preserves_dictionary_names_and_rejects_paths() {
        let config: SpellingConfig =
            toml::from_str("languages = ['en_US', 'de_DE_frami']").unwrap();
        let encoded = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<SpellingConfig>(&encoded).unwrap(), config);
        assert_eq!(config.languages()[0].as_str(), "en_US");
        assert_eq!(config.languages()[1].as_str(), "de_DE_frami");
        assert!(toml::from_str::<SpellingConfig>("languages = ['../en_US']").is_err());
    }
}

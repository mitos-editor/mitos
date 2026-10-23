//! User snippet definitions, TOML validation, and configuration-layer merging.

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use snippets::Snippet;
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

#[derive(Debug)]
pub struct UserSnippet {
    pub name: String,
    pub prefixes: Vec<String>,
    pub description: String,
    pub scopes: Vec<String>,
    pub body: String,
    pub snippet: Snippet,
    /// Display name of the configuration source, when loaded from a file.
    pub source: Option<String>,
}

#[derive(Debug, Default)]
pub struct Catalog {
    entries: Vec<Arc<UserSnippet>>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Definitions {
    #[serde(default)]
    snippet: Vec<Definition>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    name: String,
    #[serde(default)]
    prefix: Strings,
    body: Strings,
    #[serde(default)]
    description: String,
    #[serde(default)]
    scope: Strings,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Strings {
    One(String),
    Many(Vec<String>),
}

impl Default for Strings {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}
impl Strings {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

impl Catalog {
    pub fn parse(source: &str) -> Result<Self> {
        Self::parse_with_source(source, None)
    }

    /// Attach a source label for duplicate diagnostics and configuration editing.
    pub(super) fn parse_with_source(source: &str, origin: Option<&str>) -> Result<Self> {
        let definitions: Definitions = toml::from_str(source)?;
        let mut names = HashSet::new();
        let mut entries = Vec::with_capacity(definitions.snippet.len());
        for definition in definitions.snippet {
            ensure!(
                !definition.name.trim().is_empty(),
                "snippet name must not be empty"
            );
            ensure!(
                names.insert(definition.name.clone()),
                "duplicate snippet name {:?}",
                definition.name
            );
            let mut prefixes = definition.prefix.into_vec();
            ensure!(
                prefixes.iter().all(|prefix| {
                    !prefix.is_empty()
                        && prefix.chars().all(|character| {
                            !character.is_whitespace() && !character.is_control()
                        })
                }),
                "snippet {:?}: prefixes must be nonempty text without whitespace or control characters",
                definition.name
            );
            prefixes.sort();
            prefixes.dedup();
            let scopes = definition.scope.into_vec();
            ensure!(
                scopes.iter().all(|scope| !scope.trim().is_empty()),
                "snippet {:?}: language scopes must not be empty",
                definition.name
            );
            let body = definition.body.into_vec().join("\n");
            let snippet =
                Snippet::parse(&body).with_context(|| format!("snippet {:?}", definition.name))?;
            entries.push(Arc::new(UserSnippet {
                name: definition.name,
                prefixes,
                description: definition.description,
                scopes,
                body,
                source: origin.map(str::to_owned),
                snippet,
            }));
        }
        Ok(Self { entries })
    }

    /// Combine files in one configuration layer without silently shadowing names.
    pub(super) fn from_sources(sources: impl IntoIterator<Item = Catalog>) -> Result<Self> {
        let mut entries = BTreeMap::<String, Arc<UserSnippet>>::new();
        for catalog in sources {
            for entry in catalog.entries {
                if let Some(previous) = entries.get(&entry.name) {
                    anyhow::bail!(
                        "duplicate snippet name {:?} in {} and {}",
                        entry.name,
                        previous.source.as_deref().unwrap_or("<memory>"),
                        entry.source.as_deref().unwrap_or("<memory>")
                    );
                }
                entries.insert(entry.name.clone(), entry);
            }
        }
        Ok(Self {
            entries: entries.into_values().collect(),
        })
    }

    /// Project definitions replace global definitions with the same name.
    pub fn with_overrides(&self, overrides: &Self) -> Self {
        let entries: BTreeMap<_, _> = self
            .entries
            .iter()
            .chain(&overrides.entries)
            .map(|entry| (entry.name.clone(), entry.clone()))
            .collect();
        Self {
            entries: entries.into_values().collect(),
        }
    }

    pub fn matching(&self, language: &str) -> impl Iterator<Item = &Arc<UserSnippet>> {
        self.entries.iter().filter(move |entry| {
            entry.scopes.is_empty() || entry.scopes.iter().any(|scope| scope == language)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_definitions_and_filters_global_and_language_scopes() {
        let catalog = Catalog::parse(
            r#"
            [[snippet]]
            name = "global"
            prefix = ["one", "two", "one"]
            body = '${1:value}$0'
            [[snippet]]
            name = "rust"
            prefix = "rs"
            scope = ["rust", "toml"]
            body = '$TM_FILENAME'
        "#,
        )
        .unwrap();
        assert_eq!(catalog.matching("rust").count(), 2);
        assert_eq!(catalog.matching("text").count(), 1);
        assert_eq!(
            catalog.matching("text").next().unwrap().prefixes,
            ["one", "two"]
        );
        for source in [
            "unknown = true",
            "[[snippet]]\nname='x'\nprefix=''\nbody=''",
            "[[snippet]]\nname='x'\nprefix='x'\nbody=''",
        ] {
            assert!(Catalog::parse(source).is_err(), "{source}");
        }
    }
}

#[cfg(test)]
mod override_tests {
    use super::*;
    #[test]
    fn override_names_replace_the_entire_definition_and_prefixless_snippets_remain_available() {
        let global = Catalog::parse("[[snippet]]\nname='same'\nprefix='old'\nscope='first'\nbody='old'\n[[snippet]]\nname='other'\nbody='other'").unwrap();
        let project =
            Catalog::parse("[[snippet]]\nname='same'\nscope='second'\nbody='new'").unwrap();
        let merged = global.with_overrides(&project);
        assert_eq!(merged.matching("first").count(), 1);
        let entry = merged
            .matching("second")
            .find(|entry| entry.name == "same")
            .unwrap();
        assert!(entry.prefixes.is_empty());
        assert_eq!(entry.body, "new");
        assert!(Catalog::parse(
            "[[snippet]]\nname='same'\nbody='one'\n[[snippet]]\nname='same'\nbody='two'"
        )
        .is_err());
    }
}

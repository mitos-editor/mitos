//! Validated user definitions. Filesystem discovery and editor state live elsewhere.

use crate::Snippet;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use std::{collections::HashSet, sync::Arc};

#[derive(Debug)]
pub struct UserSnippet {
    pub name: String,
    pub prefixes: Vec<String>,
    pub description: String,
    pub scopes: Vec<String>,
    pub body: String,
    pub snippet: Snippet,
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
    prefix: Strings,
    body: String,
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
                !prefixes.is_empty()
                    && prefixes.iter().all(|prefix| !prefix.is_empty()
                        && prefix.chars().all(editor_core::chars::char_is_word)),
                "snippet {:?}: prefixes must be nonempty words",
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
            let snippet = Snippet::parse(&definition.body)
                .with_context(|| format!("snippet {:?}", definition.name))?;
            entries.push(Arc::new(UserSnippet {
                name: definition.name,
                prefixes,
                description: definition.description,
                scopes,
                body: definition.body,
                snippet,
            }));
        }
        Ok(Self { entries })
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

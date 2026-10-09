//! Declarative files are read through the package directory handle with explicit
//! per-file/count/aggregate budgets. No package directory becomes a runtime root.

use std::{collections::BTreeMap, path::Path};

use anyhow::{ensure, Context, Result};
use plugin_api::assets::*;

use crate::filesystem::ScopedDirectory;

pub(crate) fn load(
    package: &ScopedDirectory,
    contributions: &Contributions,
    plugin: &str,
    generation: u64,
) -> Result<(OwnedAssets, usize)> {
    ensure!(
        contributions.themes.len() <= MAX_THEMES
            && contributions.languages.len() <= MAX_LANGUAGES
            && contributions.snippets.len() <= MAX_SNIPPETS,
        "too many declarative contribution files"
    );
    let mut bytes = 0usize;
    let mut read = |path: &str, limit: usize| -> Result<String> {
        ensure!(path.len() <= 1024, "contribution path exceeds byte limit");
        let source = package.read_bounded(Path::new(path), limit)?;
        bytes = bytes
            .checked_add(source.len())
            .context("contribution byte counter overflow")?;
        ensure!(
            bytes <= MAX_ASSET_BYTES,
            "package contributions exceed 2 MiB source budget"
        );
        String::from_utf8(source).context("contribution must be UTF-8")
    };
    let mut themes = Vec::new();
    for theme in &contributions.themes {
        ensure!(valid_name(&theme.name), "invalid contributed theme name");
        themes.push(ThemeAsset {
            name: theme.name.clone(),
            source: read(&theme.path, MAX_SOURCE_BYTES)?,
        });
    }
    let mut languages = Vec::new();
    for language in &contributions.languages {
        ensure!(
            valid_name(&language.name),
            "invalid contributed language name"
        );
        let mut profile: LanguageProfile = toml::from_str(&read(&language.path, MAX_SOURCE_BYTES)?)
            .with_context(|| format!("invalid declarative language profile: {}", language.name))?;
        let mut queries = BTreeMap::new();
        for (kind, path) in std::mem::take(&mut profile.queries) {
            queries.insert(kind, read(&path, MAX_SOURCE_BYTES)?);
        }
        languages.push(LanguageAsset {
            name: language.name.clone(),
            profile,
            queries,
        });
    }
    let mut snippets = Vec::new();
    for contribution in &contributions.snippets {
        let file: SnippetFile = toml::from_str(&read(&contribution.path, MAX_ASSET_BYTES)?)
            .context("invalid static snippet contribution")?;
        ensure!(
            snippets.len() + file.snippets.len() <= MAX_SNIPPETS,
            "too many static snippets"
        );
        for snippet in file.snippets {
            snippets.push(SnippetAsset {
                language: contribution.language.clone(),
                prefix: snippet.prefix,
                body: snippet.body,
                description: snippet.description,
            });
        }
    }
    let assets = OwnedAssets {
        owner: AssetOwner {
            plugin: plugin.to_owned(),
            generation,
        },
        themes,
        languages,
        snippets,
    };
    assets.validate().context("invalid owned contributions")?;
    Ok((assets, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contribution_files_are_scoped_and_declarative_profiles_are_closed() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("theme.toml"), "inherits = 'default'").unwrap();
        std::fs::write(root.path().join("profile.toml"), "base-language = 'json'\nscope = 'source.fixture'\nextensions = ['fixture']\n[queries]\nhighlights = 'highlights.scm'").unwrap();
        std::fs::write(root.path().join("highlights.scm"), "(string) @string").unwrap();
        let directory = ScopedDirectory::open(root.path()).unwrap();
        let contributions = Contributions {
            themes: vec![ThemeContribution {
                name: "night".into(),
                path: "theme.toml".into(),
            }],
            languages: vec![LanguageContribution {
                name: "data".into(),
                path: "profile.toml".into(),
            }],
            snippets: vec![],
        };
        let (assets, bytes) = load(&directory, &contributions, "fixture", 7).unwrap();
        assert_eq!(assets.owner.generation, 7);
        assert!(bytes > 0 && assets.languages[0].profile.queries.is_empty());
        assert_eq!(
            assets.languages[0].queries[&QueryKind::Highlights],
            "(string) @string"
        );
        let escaped = Contributions {
            themes: vec![ThemeContribution {
                name: "bad".into(),
                path: "../outside.toml".into(),
            }],
            ..Default::default()
        };
        assert!(load(&directory, &escaped, "fixture", 7).is_err());
        std::fs::write(
            root.path().join("profile.toml"),
            "base-language = 'json'\nscope = 'source.fixture'\nformatter = {command = 'sh'}",
        )
        .unwrap();
        assert!(load(&directory, &contributions, "fixture", 7).is_err());
    }
}

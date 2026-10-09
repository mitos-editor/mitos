//! Prepared declarative contributions. Package discovery and compilation happen
//! on the package worker; activation only publishes owned registries and schedules
//! the existing background syntax initializer.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Weak},
};

use anyhow::{bail, ensure, Context, Result};
use editor_core::syntax::{self, config::LanguageConfiguration};
use plugin_api::assets::{AssetOwner, LanguageProfile, OwnedAssets, SnippetAsset};

use crate::{theme, Editor};

#[derive(Default)]
pub struct AssetRegistry {
    baseline: Option<Baseline>,
    installed_syntax: Weak<syntax::Loader>,
    installed_themes: Weak<theme::Loader>,
    snippets: Arc<Vec<OwnedSnippet>>,
    owners: BTreeSet<AssetOwner>,
    owned_theme_names: BTreeSet<String>,
    native_theme: Option<Arc<theme::Theme>>,
}

#[derive(Clone)]
struct Baseline {
    syntax: Arc<syntax::Loader>,
    themes: Arc<theme::Loader>,
}

#[derive(Clone)]
pub struct OwnedSnippet {
    pub owner: AssetOwner,
    pub snippet: SnippetAsset,
    pub parsed: Arc<snippets::Snippet>,
    pub rendered_bytes: usize,
    pub newlines: usize,
    pub rendered_nodes: usize,
}

/// This captured input contains only owned immutable sources and can cross the
/// worker boundary without editor references.
pub struct AssetPreparation {
    baseline: Baseline,
    packages: Vec<Arc<OwnedAssets>>,
    source_syntax: Arc<syntax::Loader>,
    source_themes: Arc<theme::Loader>,
    unchanged: bool,
    selected_theme: String,
    selected_theme_owned: bool,
    native_theme: Option<Arc<theme::Theme>>,
}

pub struct PreparedAssets {
    baseline: Baseline,
    syntax: Arc<syntax::Loader>,
    themes: Arc<theme::Loader>,
    snippets: Arc<Vec<OwnedSnippet>>,
    owners: BTreeSet<AssetOwner>,
    source_syntax: Arc<syntax::Loader>,
    source_themes: Arc<theme::Loader>,
    unchanged: bool,
    theme_styles: BTreeMap<String, theme::Theme>,
    native_theme: Option<Arc<theme::Theme>>,
}

impl AssetRegistry {
    pub fn capture(&self, editor: &Editor, packages: Vec<Arc<OwnedAssets>>) -> AssetPreparation {
        let unchanged = self.owners.is_empty()
            && packages.iter().all(|package| {
                package.themes.is_empty()
                    && package.languages.is_empty()
                    && package.snippets.is_empty()
            });
        let current = editor.syn_loader.load_full();
        let baseline = Baseline {
            syntax: self
                .baseline
                .as_ref()
                .filter(|_| {
                    self.installed_syntax
                        .upgrade()
                        .is_some_and(|installed| Arc::ptr_eq(&installed, &current))
                })
                .map_or_else(|| current.clone(), |baseline| baseline.syntax.clone()),
            themes: self
                .baseline
                .as_ref()
                .filter(|_| {
                    self.installed_themes
                        .upgrade()
                        .is_some_and(|installed| Arc::ptr_eq(&installed, &editor.theme_loader))
                })
                .map_or_else(
                    || {
                        Arc::new(theme::Loader::new(
                            editor.theme_loader.resources().without_owned(),
                        ))
                    },
                    |baseline| baseline.themes.clone(),
                ),
        };
        AssetPreparation {
            baseline,
            packages,
            source_syntax: current,
            source_themes: editor.theme_loader.clone(),
            unchanged,
            selected_theme: editor.theme.name().to_owned(),
            selected_theme_owned: self.owned_theme_names.contains(editor.theme.name())
                && self
                    .installed_themes
                    .upgrade()
                    .is_some_and(|installed| Arc::ptr_eq(&installed, &editor.theme_loader)),
            native_theme: self.native_theme.clone(),
        }
    }

    pub fn snippets(&self) -> Arc<Vec<OwnedSnippet>> {
        self.snippets.clone()
    }
    pub fn owners(&self) -> impl Iterator<Item = &AssetOwner> {
        self.owners.iter()
    }

    /// Final owned-source cleanup performs no filesystem reads or query
    /// compilation and never overwrites a later native configuration.
    pub fn restore(&mut self, editor: &mut Editor) {
        let Some(baseline) = self.baseline.take() else {
            return;
        };
        if self
            .installed_syntax
            .upgrade()
            .is_some_and(|installed| Arc::ptr_eq(&installed, &editor.syn_loader.load_full()))
        {
            editor.syn_loader.store(baseline.syntax.clone());
            refresh_document_syntax(editor, &baseline.syntax);
        }
        if self
            .installed_themes
            .upgrade()
            .is_some_and(|installed| Arc::ptr_eq(&installed, &editor.theme_loader))
        {
            editor.theme_loader = baseline.themes;
            if self.owned_theme_names.contains(editor.theme.name())
                && let Some(native) = self.native_theme.take()
            {
                let _ = editor.set_theme(Arc::unwrap_or_clone(native));
            }
        }
        self.snippets = Arc::default();
        self.owners.clear();
        self.owned_theme_names.clear();
        self.native_theme = None;
        self.installed_syntax = Weak::new();
        self.installed_themes = Weak::new();
        editor.dismiss_completions();
        editor.needs_redraw = true;
    }
}

impl Editor {
    pub fn plugin_snippets(&self) -> Arc<Vec<OwnedSnippet>> {
        self.plugins.assets.snippets()
    }
}

impl AssetPreparation {
    /// Invoke on the package worker before replacing the active manager. A failed
    /// theme, grammar, query or snippet leaves all currently active sources intact.
    pub fn prepare(self) -> Result<PreparedAssets> {
        let mut package_owners = BTreeSet::new();
        for package in &self.packages {
            package
                .validate()
                .context("invalid declarative package contributions")?;
            ensure!(
                package_owners.insert(package.owner.clone()),
                "duplicate contribution owner"
            );
        }
        if self.unchanged {
            return Ok(PreparedAssets {
                baseline: self.baseline,
                syntax: self.source_syntax.clone(),
                themes: self.source_themes.clone(),
                snippets: Arc::default(),
                owners: BTreeSet::new(),
                source_syntax: self.source_syntax,
                source_themes: self.source_themes,
                unchanged: true,
                theme_styles: BTreeMap::new(),
                native_theme: self.native_theme,
            });
        }
        let mut themes = BTreeMap::new();
        let mut queries = BTreeMap::new();
        let mut aliases = BTreeMap::new();
        let mut profiles = Vec::new();
        let mut snippets = Vec::new();
        let mut languages = BTreeMap::new();
        let mut owners = BTreeSet::new();
        for package in self.packages {
            let package = Arc::unwrap_or_clone(package);
            if !package.themes.is_empty()
                || !package.languages.is_empty()
                || !package.snippets.is_empty()
            {
                ensure!(
                    owners.insert(package.owner.clone()),
                    "duplicate contribution owner"
                );
            }
            for theme in package.themes {
                let name = format!("{}.{}", package.owner.plugin, theme.name);
                ensure!(
                    themes.insert(name, theme.source).is_none(),
                    "duplicate contributed theme"
                );
            }
            for language in package.languages {
                let profile = language.profile;
                let base = self
                    .baseline
                    .syntax
                    .language_for_name(profile.base_language.as_str())
                    .with_context(|| {
                        format!("unknown approved base language: {}", profile.base_language)
                    })?;
                let base = self.baseline.syntax.language(base).config();
                let grammar = base.grammar.as_deref().unwrap_or(&base.language_id);
                ensure!(
                    self.baseline.syntax.resources().grammar(grammar)?.is_some(),
                    "approved grammar is unavailable: {grammar}"
                );
                let name = format!("{}.{}", package.owner.plugin, language.name);
                ensure!(
                    self.baseline
                        .syntax
                        .language_for_name(name.as_str())
                        .is_none()
                        && !languages.contains_key(&name),
                    "language name conflict: {name}"
                );
                // Deserialize only this host-built closed object. No guest field
                // reaches LanguageConfiguration's executable authority settings.
                let config = safe_language_config(&name, &profile, grammar)?;
                profiles.push(config);
                aliases.insert(name.clone(), profile.base_language.clone());
                languages.insert(name.clone(), profile.base_language);
                for (kind, source) in language.queries {
                    plugin_api::query::validate(
                        &source,
                        plugin_api::query::DECLARATIVE_LIMITS,
                        plugin_api::query::Predicates::Declarative,
                    )?;
                    queries.insert((name.clone(), kind.filename().to_owned()), source);
                }
            }
            for snippet in package.snippets {
                let parsed = validate_snippet(&snippet.body)?;
                let (rendered_bytes, newlines, rendered_nodes) = parsed
                    .static_render_size(64 * 1024)
                    .context("static snippet expansion exceeds bounds or uses transforms")?;
                snippets.push(OwnedSnippet {
                    owner: package.owner.clone(),
                    snippet,
                    parsed: Arc::new(parsed),
                    rendered_bytes,
                    newlines,
                    rendered_nodes,
                });
            }
        }
        let syntax = if profiles.is_empty() && queries.is_empty() {
            self.baseline.syntax.clone()
        } else {
            let resources = self
                .baseline
                .syntax
                .resources()
                .without_owned()
                .with_owned_queries(queries, aliases);
            Arc::new(
                self.baseline
                    .syntax
                    .with_extra_languages(profiles, resources),
            )
        };
        for name in languages.keys() {
            syntax
                .validate_queries(syntax.language_for_name(name.as_str()).unwrap())
                .with_context(|| format!("invalid contributed language queries: {name}"))?;
        }
        for snippet in &snippets {
            ensure!(
                syntax
                    .language_for_name(snippet.snippet.language.as_str())
                    .is_some(),
                "unknown snippet language: {}",
                snippet.snippet.language
            );
        }
        let theme_names: Vec<_> = themes.keys().cloned().collect();
        let native_names: BTreeSet<_> = self
            .baseline
            .themes
            .resources()
            .names()
            .into_iter()
            .collect();
        let themes = Arc::new(theme::Loader::new(
            self.baseline.themes.resources().with_owned_themes(themes)?,
        ));
        // Interpret every theme before publishing, including styles/palette errors.
        let mut theme_styles = BTreeMap::new();
        for name in theme_names {
            let (style, warnings) = themes.load_with_warnings(&name)?;
            ensure!(
                warnings.is_empty(),
                "invalid contributed theme {name}: {}",
                warnings.join("; ")
            );
            if !native_names.contains(&name) {
                theme_styles.insert(name, style);
            }
        }
        let native_theme = if self.selected_theme_owned {
            self.native_theme
                .unwrap_or_else(|| Arc::new(themes.default()))
        } else {
            self.baseline
                .themes
                .load(&self.selected_theme)
                .map(Arc::new)
                .unwrap_or_else(|_| {
                    self.native_theme
                        .unwrap_or_else(|| Arc::new(themes.default()))
                })
        };
        Ok(PreparedAssets {
            baseline: self.baseline,
            syntax,
            themes,
            snippets: Arc::new(snippets),
            owners,
            source_syntax: self.source_syntax,
            source_themes: self.source_themes,
            unchanged: false,
            theme_styles,
            native_theme: Some(native_theme),
        })
    }
}

fn safe_language_config(
    name: &str,
    profile: &LanguageProfile,
    grammar: &str,
) -> Result<LanguageConfiguration> {
    Ok(serde_json::from_value(serde_json::json!({
        "name": name, "scope": profile.scope, "file-types": profile.extensions, "grammar": grammar,
        "language-servers": [], "auto-format": false,
    }))?)
}

impl PreparedAssets {
    pub fn is_current(&self, editor: &Editor) -> bool {
        Arc::ptr_eq(&self.source_syntax, &editor.syn_loader.load_full())
            && Arc::ptr_eq(&self.source_themes, &editor.theme_loader)
    }

    pub fn activate(self, registry: &mut AssetRegistry, editor: &mut Editor) -> Result<()> {
        ensure!(
            self.is_current(editor),
            "native resource configuration changed during plugin preparation"
        );
        if self.unchanged {
            return Ok(());
        }
        let selected_theme = editor.theme.name().to_owned();
        let previously_owned = registry.owned_theme_names.contains(&selected_theme)
            && registry
                .installed_themes
                .upgrade()
                .is_some_and(|installed| Arc::ptr_eq(&installed, &editor.theme_loader));
        let native_theme = if previously_owned {
            self.native_theme
        } else {
            // Preserve a native theme chosen while preparation was running.
            Some(Arc::new(editor.theme.clone()))
        };
        let selected_style =
            self.theme_styles.get(&selected_theme).cloned().or_else(|| {
                previously_owned.then(|| native_theme.as_ref().unwrap().as_ref().clone())
            });
        let syntax_changed = !Arc::ptr_eq(&self.syntax, &editor.syn_loader.load_full());
        editor.syn_loader.store(self.syntax.clone());
        editor.theme_loader = self.themes;
        if syntax_changed {
            refresh_document_syntax(editor, &self.syntax);
        }
        if let Some(style) = selected_style {
            let _ = editor.set_theme(style);
        }
        editor.dismiss_completions();
        editor.needs_redraw = true;
        registry.baseline = Some(self.baseline);
        registry.installed_syntax = Arc::downgrade(&self.syntax);
        registry.installed_themes = Arc::downgrade(&editor.theme_loader);
        registry.snippets = self.snippets;
        registry.owners = self.owners;
        registry.owned_theme_names = self.theme_styles.into_keys().collect();
        registry.native_theme = native_theme;
        Ok(())
    }
}

fn refresh_document_syntax(editor: &mut Editor, syntax: &Arc<syntax::Loader>) {
    for document in editor.documents_mut() {
        let current = document.language_name().map(str::to_owned);
        // Removed profiles return to native filename/shebang detection;
        // their approved grammar is not a native provider configuration.
        let name = current
            .as_deref()
            .filter(|name| syntax.language_for_name(*name).is_some());
        document.language = name
            .and_then(|name| syntax.language_for_name(name))
            .map(|language| syntax.language(language).config().clone())
            .or_else(|| document.detect_language_config(syntax));
        document.initialize_syntax(syntax.clone());
    }
}

fn validate_snippet(body: &str) -> Result<snippets::Snippet> {
    let mut depth = 0usize;
    let mut escaped = false;
    for character in body.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '{' => {
                depth += 1;
                ensure!(depth <= 32, "snippet nesting exceeds bound");
            }
            '}' => depth = depth.saturating_sub(1),
            character if character.is_control() && !matches!(character, '\n' | '\r' | '\t') => {
                bail!("snippet contains terminal control characters")
            }
            _ => (),
        }
    }
    snippets::Snippet::parse(body).map_err(|_| anyhow::anyhow!("invalid snippet syntax"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_profiles_cannot_inherit_base_provider_or_process_authority() {
        let base: LanguageConfiguration = serde_json::from_value(serde_json::json!({
            "name":"rust", "scope":"source.rust", "file-types":["rs"], "grammar":"rust",
            "formatter":{"command":"sh","args":["-c","touch forbidden"]},
            "language-servers":["host-lsp"], "auto-format":true, "code-actions-on-save":["source.fixAll"],
            "workspace-lsp-roots":["private"],
        })).unwrap();
        assert!(base.formatter.is_some());
        assert!(!base.language_servers.is_empty());
        let profile = LanguageProfile {
            base_language: "rust".into(),
            scope: "source.fixture".into(),
            extensions: vec!["fixture".into()],
            queries: BTreeMap::new(),
        };
        let contributed =
            safe_language_config("fixture.custom", &profile, base.grammar.as_deref().unwrap())
                .unwrap();
        assert_eq!(contributed.grammar.as_deref(), Some("rust"));
        assert!(contributed.formatter.is_none());
        assert!(contributed.language_servers.is_empty());
        assert!(contributed.debugger.is_none());
        assert!(contributed.code_actions_on_save.is_none());
        assert!(contributed.workspace_lsp_roots.is_none());
        assert_eq!(contributed.auto_format, Some(false));
    }
    #[test]
    fn snippet_preflight_bounds_recursion_before_parsing() {
        assert!(validate_snippet("${1:hello ${2:world}}$0").is_ok());
        assert!(validate_snippet(&format!("{}x{}", "${1:".repeat(33), "}".repeat(33))).is_err());
        assert!(validate_snippet("\x1b[31m").is_err());
    }
}

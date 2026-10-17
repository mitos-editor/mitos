//! Grammar definitions and loading of compiled runtime libraries.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};
use tree_house::tree_sitter::Grammar;

#[cfg(target_os = "macos")]
pub const DYLIB_EXTENSION: &str = "dylib";

#[cfg(all(unix, not(target_os = "macos")))]
pub const DYLIB_EXTENSION: &str = "so";

#[cfg(windows)]
pub const DYLIB_EXTENSION: &str = "dll";

#[cfg(target_arch = "wasm32")]
pub const DYLIB_EXTENSION: &str = "wasm";

#[derive(Debug, Serialize, Deserialize)]
struct Configuration {
    #[serde(rename = "use-grammars")]
    pub grammar_selection: Option<GrammarSelection>,
    pub grammar: Vec<GrammarConfiguration>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", untagged)]
pub enum GrammarSelection {
    Only { only: HashSet<String> },
    Except { except: HashSet<String> },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrammarConfiguration {
    #[serde(rename = "name")]
    pub grammar_id: String,
    pub source: GrammarSource,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", untagged)]
pub enum GrammarSource {
    Local {
        path: String,
    },
    Git {
        #[serde(rename = "git")]
        remote: String,
        #[serde(rename = "rev")]
        revision: String,
        subpath: Option<String>,
    },
}

/// Load a grammar from the process-default runtime directories.
pub fn get_language(name: &str) -> Result<Option<Grammar>> {
    crate::syntax::Resources::default().grammar(name)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn load_language(_name: &str, _path: &Path) -> Result<Option<Grammar>> {
    unimplemented!()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn load_language(name: &str, library_path: &Path) -> Result<Option<Grammar>> {
    if !library_path.exists() {
        return Ok(None);
    }
    let grammar = unsafe { Grammar::new(name, library_path) }?;
    Ok(Some(grammar))
}

/// Print a notice if the current workspace has a `.mitos/languages.toml` that we *would* have
/// merged but the workspace-trust gate is keeping us from.
fn warn_if_workspace_languages_skipped(trust: &workspace::trust::WorkspaceTrust) {
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

// Returns the set of grammar configurations the user requests.
// Grammars are configured in the default and user `languages.toml` and are
// merged. The `grammar_selection` key of the config is then used to filter
// down all grammars into a subset of the user's choosing.
pub fn get_grammar_configs() -> Result<Vec<GrammarConfiguration>> {
    // `--grammar fetch/build` clones grammar sources from URLs in `languages.toml` and compiles
    // them into `.so` files mitos later loads at runtime. If we let workspace
    // `.mitos/languages.toml` in through `fully_trusted`, a malicious workspace could inject a
    // grammar with an attacker-controlled git source — running grammar build in that
    // directory would clone and compile attacker code
    let trust =
        workspace::trust::WorkspaceTrust::new(Default::default(), crate::workspace_trust_dir());
    warn_if_workspace_languages_skipped(&trust);
    let config: Configuration = crate::config::user_lang_config(&trust)
        .context("Could not parse languages.toml")?
        .try_into()?;

    let grammars = match config.grammar_selection {
        Some(GrammarSelection::Only { only: selections }) => config
            .grammar
            .into_iter()
            .filter(|grammar| selections.contains(&grammar.grammar_id))
            .collect(),
        Some(GrammarSelection::Except { except: rejections }) => config
            .grammar
            .into_iter()
            .filter(|grammar| !rejections.contains(&grammar.grammar_id))
            .collect(),
        None => config.grammar,
    };

    Ok(grammars)
}

pub fn get_grammar_names() -> Result<Option<HashSet<String>>> {
    // See `get_grammar_configs`, same threat: workspace-local
    // `languages.toml` must not influence the grammar set without
    // explicit on-disk trust.
    let trust =
        workspace::trust::WorkspaceTrust::new(Default::default(), crate::workspace_trust_dir());
    warn_if_workspace_languages_skipped(&trust);
    let config: Configuration = crate::config::user_lang_config(&trust)
        .context("Could not parse languages.toml")?
        .try_into()?;

    let grammars = match config.grammar_selection {
        Some(GrammarSelection::Only { only: selections }) => Some(selections),
        Some(GrammarSelection::Except { except: rejections }) => Some(
            config
                .grammar
                .into_iter()
                .map(|grammar| grammar.grammar_id)
                .filter(|id| !rejections.contains(id))
                .collect(),
        ),
        None => None,
    };

    Ok(grammars)
}

/// Gives the contents of a file from a language's `runtime/queries/<lang>`
/// directory
pub fn load_runtime_file(language: &str, filename: &str) -> Result<String, std::io::Error> {
    crate::syntax::Resources::default().query_file(language, filename)
}

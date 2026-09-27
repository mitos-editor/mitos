//! Discovery and loading of Mitos configuration and runtime assets.
//!
//! Runtime assets (queries, themes, grammars, and language configuration) may
//! come from several directories. [`runtime_dirs`] exposes their precedence;
//! lookups must search every directory in that order because a higher-priority
//! directory may override only part of the runtime.
//!
//! The configuration and log file paths are process-wide values. Startup must
//! call [`initialize_config_file`] and [`initialize_log_file`] before calling
//! [`config_file`] or [`log_file`]. The initializers are intentionally
//! first-write-wins so later application components cannot silently change the
//! paths selected from command-line arguments.
//!
//! [`syntax`] selects grammar and query sources from ordered runtime paths;
//! [`grammar`] builds and loads native tree-sitter libraries.
//! [`theme`] discovers theme sources and resolves their inheritance.
//! [`workspace_trust`] keeps decisions about executing workspace-controlled
//! programs separate from path discovery.

pub mod config;
pub mod grammar;
pub mod syntax;
pub mod theme;
pub mod workspace_trust;

use stdx::{env::current_working_dir, path};

use etcetera::base_strategy::{choose_base_strategy, BaseStrategy};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};

/// Build version, optionally followed by the source revision embedded by
/// `build.rs`.
pub const VERSION_AND_GIT_HASH: &str = env!("VERSION_AND_GIT_HASH");

static RUNTIME_DIRS: LazyLock<Vec<PathBuf>> = LazyLock::new(prioritize_runtime_dirs);

static CONFIG_FILE: OnceLock<PathBuf> = OnceLock::new();

static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();

/// Selects the process-wide configuration file path.
///
/// The parent directory is created when possible. Only the first call changes
/// the stored path; subsequent calls are ignored.
pub fn initialize_config_file(specified_file: Option<PathBuf>) {
    let config_file = specified_file.unwrap_or_else(default_config_file);
    ensure_parent_dir(&config_file);
    CONFIG_FILE.set(config_file).ok();
}

/// Selects the process-wide log file path.
///
/// The parent directory is created when possible. Only the first call changes
/// the stored path; subsequent calls are ignored.
pub fn initialize_log_file(specified_file: Option<PathBuf>) {
    let log_file = specified_file.unwrap_or_else(default_log_file);
    ensure_parent_dir(&log_file);
    LOG_FILE.set(log_file).ok();
}

/// A list of runtime directories from highest to lowest priority
///
/// The priority is:
///
/// 1. `runtime` directory in the Cargo workspace
/// 2. subdirectory of user config directory (always included)
/// 3. `MITOS_RUNTIME` (if environment variable is set)
/// 4. `MITOS_DEFAULT_RUNTIME` (if environment variable is set *at build time*)
/// 5. subdirectory of path to mitos executable (always included)
///
/// Postcondition: returns at least two paths (they might not exist).
fn prioritize_runtime_dirs() -> Vec<PathBuf> {
    const RT_DIR: &str = "runtime";
    // Adding higher priority first
    let mut rt_dirs = Vec::new();
    if let Some(dir) = option_env!("CARGO_MANIFEST_DIR") {
        // Product crates live under `crates/`, two levels below the workspace root.
        let path = PathBuf::from(dir)
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(RT_DIR);
        log::debug!("runtime dir: {}", path.to_string_lossy());
        rt_dirs.push(path);
    }

    let conf_rt_dir = config_dir().join(RT_DIR);
    rt_dirs.push(conf_rt_dir);

    if let Ok(dir) = std::env::var("MITOS_RUNTIME") {
        let dir = path::expand_tilde(Path::new(&dir));
        rt_dirs.push(path::normalize(dir));
    }

    // If this variable is set during build time, it will always be included
    // in the lookup list. This allows downstream packagers to set a fallback
    // directory to a location that is conventional on their distro so that they
    // need not resort to a wrapper script or a global environment variable.
    if let Some(dir) = std::option_env!("MITOS_DEFAULT_RUNTIME") {
        rt_dirs.push(dir.into());
    }

    // fallback to location of the executable being run
    // canonicalize the path in case the executable is symlinked
    let exe_rt_dir = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .and_then(|path| path.parent().map(|path| path.to_path_buf().join(RT_DIR)))
        .unwrap();
    rt_dirs.push(exe_rt_dir);
    rt_dirs
}

/// Runtime directories ordered from highest to lowest priority
///
/// All directories should be checked when looking for files.
///
/// Postcondition: returns at least one path (it might not exist).
pub fn runtime_dirs() -> &'static [PathBuf] {
    &RUNTIME_DIRS
}

/// Find a runtime file in process-default priority order, retaining the final
/// attempted path when no file exists.
pub fn runtime_file(rel_path: impl AsRef<Path>) -> PathBuf {
    runtime_file_in(runtime_dirs(), rel_path.as_ref())
}

pub(crate) fn runtime_file_in(directories: &[PathBuf], relative: &Path) -> PathBuf {
    directories
        .iter()
        .map(|dir| dir.join(relative))
        .find(|path| path.exists())
        .unwrap_or_else(|| {
            directories
                .last()
                .map(|dir| dir.join(relative))
                .unwrap_or_default()
        })
}

pub fn config_dir() -> PathBuf {
    // TODO: allow env var override
    let strategy = choose_base_strategy().expect("Unable to find the config directory!");
    let mut path = strategy.config_dir();
    path.push("mitos");
    path
}

pub fn cache_dir() -> PathBuf {
    // TODO: allow env var override
    let strategy = choose_base_strategy().expect("Unable to find the cache directory!");
    let mut path = strategy.cache_dir();
    path.push("mitos");
    path
}

pub fn state_dir() -> PathBuf {
    let strategy = choose_base_strategy().expect("Unable to find the state directory!");
    // Windows has no dedicated state directory. Keep persistent state in its
    // data directory rather than requiring an XDG-only location.
    let mut path = strategy.state_dir().unwrap_or_else(|| strategy.data_dir());
    path.push("mitos");
    path
}

pub fn data_dir() -> PathBuf {
    let strategy = choose_base_strategy().expect("Unable to find the data directory!");
    let mut path = strategy.data_dir();
    path.push("mitos");
    path
}

/// Returns the configuration file selected during startup.
///
/// # Panics
///
/// Panics if [`initialize_config_file`] has not been called.
pub fn config_file() -> PathBuf {
    CONFIG_FILE.get().map(|path| path.to_path_buf()).unwrap()
}

/// Returns the log file selected during startup.
///
/// # Panics
///
/// Panics if [`initialize_log_file`] has not been called.
pub fn log_file() -> PathBuf {
    LOG_FILE.get().map(|path| path.to_path_buf()).unwrap()
}

pub fn workspace_config_file() -> PathBuf {
    find_workspace().0.join(".mitos").join("config.toml")
}

pub fn workspace_lang_config_file() -> PathBuf {
    find_workspace().0.join(".mitos").join("languages.toml")
}

pub fn lang_config_file() -> PathBuf {
    config_dir().join("languages.toml")
}

pub fn default_log_file() -> PathBuf {
    cache_dir().join("mitos.log")
}

/// The personal dictionary for a spelling `language` (e.g. `"en_US"`): the words the user has added
/// via "Add to dictionary", one per line. Namespaced per language so a word added for one language
/// isn't accepted in another.
pub fn personal_dictionary_file(language: &str) -> PathBuf {
    state_dir()
        .join("dictionaries")
        .join(format!("{language}.txt"))
}

/// User spelling ignores, one word per line, kept separate from dictionary suggestions.
pub fn spelling_ignore_file(language: &str) -> PathBuf {
    config_dir()
        .join("spelling")
        .join(format!("{language}.ignore"))
}

/// Merge two TOML documents, merging values from `right` onto `left`
///
/// `merge_depth` sets the nesting depth up to which values are merged instead
/// of overridden.
///
/// When a table exists in both `left` and `right`, the merged table consists of
/// all keys in `left`'s table unioned with all keys in `right` with the values
/// of `right` being merged recursively onto values of `left`.
///
/// `crate::merge_toml_values(a, b, 3)` combines, for example:
///
/// b:
/// ```toml
/// [[language]]
/// name = "toml"
/// language-server = { command = "taplo", args = ["lsp", "stdio"] }
/// ```
/// a:
/// ```toml
/// [[language]]
/// language-server = { command = "/usr/bin/taplo" }
/// ```
///
/// into:
/// ```toml
/// [[language]]
/// name = "toml"
/// language-server = { command = "/usr/bin/taplo" }
/// ```
///
/// thus it overrides the third depth-level of b with values of a if they exist,
/// but otherwise merges their values
pub fn merge_toml_values(left: toml::Value, right: toml::Value, merge_depth: usize) -> toml::Value {
    use toml::Value;

    fn get_name(v: &Value) -> Option<&str> {
        v.get("name").and_then(Value::as_str)
    }

    match (left, right) {
        (Value::Array(mut left_items), Value::Array(right_items)) => {
            if merge_depth > 0 {
                left_items.reserve(right_items.len());
                for rvalue in right_items {
                    let lvalue = get_name(&rvalue)
                        .and_then(|rname| {
                            left_items.iter().position(|v| get_name(v) == Some(rname))
                        })
                        .map(|lpos| left_items.remove(lpos));
                    let mvalue = match lvalue {
                        Some(lvalue) => merge_toml_values(lvalue, rvalue, merge_depth - 1),
                        None => rvalue,
                    };
                    left_items.push(mvalue);
                }
                Value::Array(left_items)
            } else {
                Value::Array(right_items)
            }
        }
        (Value::Table(mut left_map), Value::Table(right_map)) => {
            if merge_depth > 0 {
                for (rname, rvalue) in right_map {
                    match left_map.remove(&rname) {
                        Some(lvalue) => {
                            let merged_value = merge_toml_values(lvalue, rvalue, merge_depth - 1);
                            left_map.insert(rname, merged_value);
                        }
                        None => {
                            left_map.insert(rname, rvalue);
                        }
                    }
                }
                Value::Table(left_map)
            } else {
                Value::Table(right_map)
            }
        }
        // Catch everything else we didn't handle, and use the right value
        (_, value) => value,
    }
}

/// Finds the current workspace folder.
/// Used as a ceiling dir for LSP root resolution, the filepicker and potentially as a future filewatching root
///
/// This function starts searching the FS upward from the CWD
/// and returns the first directory that contains either `.git`, `.svn`, `.jj` or `.mitos`.
/// If no workspace was found returns (CWD, true).
/// Otherwise (workspace, false) is returned
pub fn find_workspace() -> (PathBuf, bool) {
    let current_dir = current_working_dir();
    find_workspace_in(current_dir)
}

pub fn find_workspace_in(dir: impl AsRef<Path>) -> (PathBuf, bool) {
    let dir = dir.as_ref();
    for ancestor in dir.ancestors() {
        if ancestor.join(".git").exists()
            || ancestor.join(".svn").exists()
            || ancestor.join(".jj").exists()
            || ancestor.join(".mitos").exists()
        {
            return (ancestor.to_owned(), false);
        }
    }

    (dir.to_owned(), true)
}

fn default_config_file() -> PathBuf {
    config_dir().join("config.toml")
}

fn ensure_parent_dir(path: &Path) {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).ok();
    }
}

#[cfg(test)]
mod directory_tests {
    #[test]
    fn state_directory_is_absolute_and_namespaced() {
        let path = super::state_dir();
        assert!(path.is_absolute());
        assert!(path.ends_with("mitos"));
        #[cfg(windows)]
        assert_eq!(path, super::data_dir());
    }
}

#[cfg(test)]
mod merge_toml_tests {
    use std::str;

    use super::merge_toml_values;
    use toml::Value;

    #[test]
    fn language_toml_map_merges() {
        const USER: &str = r#"
        [[language]]
        name = "nix"
        test = "bbb"
        indent = { tab-width = 4, unit = "    ", test = "aaa" }
        "#;

        let base = include_bytes!("../../../languages.toml");
        let base = str::from_utf8(base).expect("Couldn't parse built-in languages config");
        let base: Value = toml::from_str(base).expect("Couldn't parse built-in languages config");
        let user: Value = toml::from_str(USER).unwrap();

        let merged = merge_toml_values(base, user, 3);
        let languages = merged.get("language").unwrap().as_array().unwrap();
        let nix = languages
            .iter()
            .find(|v| v.get("name").unwrap().as_str().unwrap() == "nix")
            .unwrap();
        let nix_indent = nix.get("indent").unwrap();

        // We changed tab-width and unit in indent so check them if they are the new values
        assert_eq!(
            nix_indent.get("tab-width").unwrap().as_integer().unwrap(),
            4
        );
        assert_eq!(nix_indent.get("unit").unwrap().as_str().unwrap(), "    ");
        // We added a new keys, so check them
        assert_eq!(nix.get("test").unwrap().as_str().unwrap(), "bbb");
        assert_eq!(nix_indent.get("test").unwrap().as_str().unwrap(), "aaa");
        // We didn't change comment-token so it should be same
        assert_eq!(nix.get("comment-token").unwrap().as_str().unwrap(), "#");
    }

    #[test]
    fn language_toml_nested_array_merges() {
        const USER: &str = r#"
        [[language]]
        name = "typescript"
        language-server = { command = "deno", args = ["lsp"] }
        "#;

        let base = include_bytes!("../../../languages.toml");
        let base = str::from_utf8(base).expect("Couldn't parse built-in languages config");
        let base: Value = toml::from_str(base).expect("Couldn't parse built-in languages config");
        let user: Value = toml::from_str(USER).unwrap();

        let merged = merge_toml_values(base, user, 3);
        let languages = merged.get("language").unwrap().as_array().unwrap();
        let ts = languages
            .iter()
            .find(|v| v.get("name").unwrap().as_str().unwrap() == "typescript")
            .unwrap();
        assert_eq!(
            ts.get("language-server")
                .unwrap()
                .get("args")
                .unwrap()
                .as_array()
                .unwrap(),
            &vec![Value::String("lsp".into())]
        )
    }
}

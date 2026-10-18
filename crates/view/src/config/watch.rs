use serde::{Deserialize, Serialize};

/// WatchConfig for file watching
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct WatchConfig {
    /// Enable recursive native file watching.
    pub enable: bool,
    pub watch_vcs: bool,
    /// Only enable the file watcher inside Mitos workspaces (VCS repos and directories with .mitos
    /// directory) this prevents watching large directories like $HOME by default
    ///
    /// Defaults to `true`
    pub require_workspace: bool,
    /// Enables ignoring hidden files.
    pub hidden: bool,
    /// Enables reading `.ignore` files.
    pub ignore: bool,
    /// Enables reading `.gitignore` files.
    pub git_ignore: bool,
    /// Enables reading global .gitignore, whose path is specified in git's config: `core.excludefile` option.
    pub git_global: bool,
    /// Maximum depth below a watch root; deeper open files use polling.
    pub max_depth: Option<usize>,
}

impl Default for WatchConfig {
    fn default() -> Self {
        WatchConfig {
            enable: true,
            watch_vcs: true,
            require_workspace: true,
            hidden: true,
            ignore: true,
            git_ignore: true,
            git_global: true,
            max_depth: Some(10),
        }
    }
}

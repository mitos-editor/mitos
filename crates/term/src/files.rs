//! Translate frontend file-discovery settings into the shared traversal policy.

pub(crate) fn picker_options(config: &editor_config::FilePickerConfig) -> search::Options {
    search::Options {
        hidden: config.hidden,
        parents: config.parents,
        ignore: config.ignore,
        require_git: config.require_git,
        follow_symlinks: config.follow_symlinks,
        deduplicate_links: config.deduplicate_links,
        git_ignore: config.git_ignore,
        git_global: config.git_global,
        git_exclude: config.git_exclude,
        max_depth: config.max_depth,
        custom_ignore_files: ignore_files(),
        ..search::Options::default()
    }
}

pub(crate) fn explorer_options(config: &editor_config::FileExplorerConfig) -> search::Options {
    search::Options {
        hidden: config.hidden,
        parents: config.parents,
        ignore: config.ignore,
        require_git: config.require_git,
        follow_symlinks: config.follow_symlinks,
        git_ignore: config.git_ignore,
        git_global: config.git_global,
        git_exclude: config.git_exclude,
        // The explorer lists one level, including metadata directories and aliases.
        exclude_vcs: false,
        deduplicate_links: false,
        max_depth: Some(1),
        custom_ignore_files: ignore_files(),
    }
}

fn ignore_files() -> Vec<std::path::PathBuf> {
    vec![loader::config_dir().join("ignore"), ".mitos/ignore".into()]
}

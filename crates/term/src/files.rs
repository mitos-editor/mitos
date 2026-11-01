//! Translate frontend file-discovery settings into the shared traversal policy.

pub(crate) fn picker_options(config: &editor_config::FilePickerConfig) -> search::Options {
    search::Options {
        hidden: config.hidden,
        parents: config.parents,
        ignore: config.ignore,
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

mod picker;
pub(crate) use picker::FilePickerCache;

/// We want to exclude files that the editor can't handle yet
pub(crate) fn excluded_types() -> ignore::types::Types {
    use ignore::types::TypesBuilder;
    let mut type_builder = TypesBuilder::new();
    type_builder
        .add(
            "compressed",
            "*.{zip,gz,bz2,zst,lzo,sz,tgz,tbz2,lz,lz4,lzma,lzo,z,Z,xz,7z,rar,cab}",
        )
        .expect("Invalid type definition");
    type_builder.negate("all");
    type_builder
        .build()
        .expect("failed to build excluded_types")
}

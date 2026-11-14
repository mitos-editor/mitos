use std::path::Path;

use globset::{GlobBuilder, GlobSet};

use crate::lsp;

#[derive(Default, Debug)]
pub(crate) struct FileOperationFilter {
    dir_globs: GlobSet,
    file_globs: GlobSet,
}

impl FileOperationFilter {
    fn new(capability: Option<&lsp::FileOperationRegistrationOptions>) -> FileOperationFilter {
        let Some(cap) = capability else {
            return FileOperationFilter::default();
        };
        let mut dir_globs = GlobSet::builder();
        let mut file_globs = GlobSet::builder();
        for filter in &cap.filters {
            // TODO: support other url schemes
            let is_non_file_schema = filter
                .scheme
                .as_ref()
                .is_some_and(|schema| schema != "file");
            if is_non_file_schema {
                continue;
            }
            let ignore_case = filter
                .pattern
                .options
                .as_ref()
                .and_then(|opts| opts.ignore_case)
                .unwrap_or(false);
            let mut glob_builder = GlobBuilder::new(&filter.pattern.glob);
            glob_builder.case_insensitive(ignore_case);
            let glob = match glob_builder.build() {
                Ok(glob) => glob,
                Err(err) => {
                    log::error!("invalid glob send by LS: {err}");
                    continue;
                }
            };
            match filter.pattern.matches {
                Some(lsp::FileOperationPatternKind::File) => {
                    file_globs.add(glob);
                }
                Some(lsp::FileOperationPatternKind::Folder) => {
                    dir_globs.add(glob);
                }
                None => {
                    file_globs.add(glob.clone());
                    dir_globs.add(glob);
                }
            };
        }
        let file_globs = file_globs.build().unwrap_or_else(|err| {
            log::error!("invalid globs send by LS: {err}");
            GlobSet::empty()
        });
        let dir_globs = dir_globs.build().unwrap_or_else(|err| {
            log::error!("invalid globs send by LS: {err}");
            GlobSet::empty()
        });
        FileOperationFilter {
            dir_globs,
            file_globs,
        }
    }

    pub(crate) fn has_interest(&self, path: &Path, is_dir: bool) -> bool {
        if is_dir {
            self.dir_globs.is_match(path)
        } else {
            self.file_globs.is_match(path)
        }
    }
}

#[derive(Default, Debug)]
pub(crate) struct FileOperationsInterest {
    pub did_create: FileOperationFilter,
    pub will_create: FileOperationFilter,
    pub did_rename: FileOperationFilter,
    pub will_rename: FileOperationFilter,
    pub did_delete: FileOperationFilter,
    pub will_delete: FileOperationFilter,
}

impl FileOperationsInterest {
    pub fn new(capabilities: &lsp::ServerCapabilities) -> FileOperationsInterest {
        let capabilities = capabilities
            .workspace
            .as_ref()
            .and_then(|capabilities| capabilities.file_operations.as_ref());
        let Some(capabilities) = capabilities else {
            return FileOperationsInterest::default();
        };
        FileOperationsInterest {
            did_create: FileOperationFilter::new(capabilities.did_create.as_ref()),
            will_create: FileOperationFilter::new(capabilities.will_create.as_ref()),
            did_rename: FileOperationFilter::new(capabilities.did_rename.as_ref()),
            will_rename: FileOperationFilter::new(capabilities.will_rename.as_ref()),
            did_delete: FileOperationFilter::new(capabilities.did_delete.as_ref()),
            will_delete: FileOperationFilter::new(capabilities.will_delete.as_ref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_respects_ignore_case() {
        for options in [
            None,
            Some(lsp::FileOperationPatternOptions { ignore_case: None }),
            Some(lsp::FileOperationPatternOptions {
                ignore_case: Some(false),
            }),
            Some(lsp::FileOperationPatternOptions {
                ignore_case: Some(true),
            }),
        ] {
            let ignore_case = options.as_ref().and_then(|options| options.ignore_case);
            let capability = lsp::FileOperationRegistrationOptions {
                filters: vec![lsp::FileOperationFilter {
                    scheme: Some("file".into()),
                    pattern: lsp::FileOperationPattern {
                        glob: "**/src/*.rs".into(),
                        matches: None,
                        options,
                    },
                }],
            };
            let filter = FileOperationFilter::new(Some(&capability));

            for is_dir in [false, true] {
                assert!(filter.has_interest(Path::new("project/src/main.rs"), is_dir));
                assert_eq!(
                    filter.has_interest(Path::new("project/Src/Main.RS"), is_dir),
                    ignore_case == Some(true),
                    "ignore_case={ignore_case:?}, is_dir={is_dir}"
                );
                assert!(!filter.has_interest(Path::new("project/tests/main.rs"), is_dir));
            }
        }
    }

    #[test]
    fn filter_respects_scheme_and_pattern_kind() {
        for scheme in [None, Some("file"), Some("untitled")] {
            for kind in [
                None,
                Some(lsp::FileOperationPatternKind::File),
                Some(lsp::FileOperationPatternKind::Folder),
            ] {
                let capability = lsp::FileOperationRegistrationOptions {
                    filters: vec![lsp::FileOperationFilter {
                        scheme: scheme.map(str::to_owned),
                        pattern: lsp::FileOperationPattern {
                            glob: "**/src/*".into(),
                            matches: kind.clone(),
                            options: None,
                        },
                    }],
                };
                let filter = FileOperationFilter::new(Some(&capability));

                for is_dir in [false, true] {
                    let matches_kind = match kind {
                        None => true,
                        Some(lsp::FileOperationPatternKind::File) => !is_dir,
                        Some(lsp::FileOperationPatternKind::Folder) => is_dir,
                    };
                    assert_eq!(
                        filter.has_interest(Path::new("project/src/main.rs"), is_dir),
                        scheme != Some("untitled") && matches_kind,
                        "scheme={scheme:?}, kind={kind:?}, is_dir={is_dir}"
                    );
                }
            }
        }
    }
}

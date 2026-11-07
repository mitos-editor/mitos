//! Cached workspace search with matching rows and their parent chains.

use super::{content, ExplorerData, ExplorerItem};
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use view::Editor;

type ExplorerRows = Vec<(ExplorerItem, usize)>;

#[derive(Clone, Default)]
pub struct ExplorerTree {
    pub expanded: ui_core::tree::Tree<PathBuf>,
    pub searching: bool,
    // Reuse one recursive listing across keystrokes. Background filesystem
    // refreshes replace it so subsequent queries see the same source revision.
    pub index: Arc<Mutex<Option<ExplorerRows>>>,
}

impl ExplorerTree {
    pub fn clear_index(&self) {
        *self.index.lock().unwrap() = None;
    }

    pub fn is_expanded(&self, path: &PathBuf) -> bool {
        self.searching || self.expanded.is_expanded(path)
    }

    pub fn reveal(&mut self, root: &Path, path: &Path) {
        for parent in path
            .ancestors()
            .skip(1)
            .take_while(|parent| *parent != root)
        {
            let parent = parent.to_path_buf();
            if !self.expanded.is_expanded(&parent) {
                self.expanded.toggle(parent);
            }
        }
    }
}

pub(super) fn query(
    editor: &Editor,
    query: &str,
    data: &ExplorerData,
    selected: Option<&ExplorerItem>,
) -> anyhow::Result<(Vec<ExplorerItem>, ExplorerData, usize)> {
    let mut data = data.clone();
    let tree = data.tree.as_mut().unwrap();
    tree.searching = !query.is_empty();
    let (options, cursor) = if query.is_empty() {
        if let Some((path, _)) = selected {
            tree.reveal(&data.root, path);
        }
        let options = content(
            &data.root,
            Some(tree),
            &editor.config().file_explorer,
            || false,
        )?;
        let cursor = selected
            .and_then(|selected| options.iter().position(|item| item == selected))
            .unwrap_or(0);
        (options, cursor)
    } else {
        let index = tree.index.lock().unwrap();
        let (options, first_match) =
            matches(&data.root, query, index.as_deref().unwrap_or_default());
        let cursor = selected
            .and_then(|selected| options.iter().position(|item| item == selected))
            .unwrap_or(first_match);
        (options, cursor)
    };
    Ok((options, data, cursor))
}

pub(in crate::ui) fn search_index(
    root: &Path,
    config: &editor_config::FileExplorerConfig,
    cancelled: impl Fn() -> bool,
) -> std::io::Result<Vec<(ExplorerItem, usize)>> {
    std::fs::read_dir(root)?;
    let mut options = crate::files::explorer_options(config);
    options.exclude_vcs = true;
    let mut rows: Vec<_> = options
        .walker(root)
        .max_depth(None)
        .types(crate::files::excluded_types())
        .build()
        .take_while(|_| !cancelled())
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path() != root)
        .map(|entry| {
            let is_dir = search::entry_file_type(&entry).is_some_and(|kind| kind.is_dir());
            let depth = entry.depth().saturating_sub(1);
            ((entry.into_path(), is_dir), depth)
        })
        .collect();
    // Order each directory's children with directories first, keeping every
    // subtree together. Cache keys to avoid filesystem reads while sorting.
    rows.sort_by_cached_key(|((path, is_dir), _)| {
        let mut key: Vec<_> = path
            .strip_prefix(root)
            .unwrap_or(path)
            .components()
            .map(|part| (false, part.as_os_str().to_os_string()))
            .collect();
        if let Some((is_file, _)) = key.last_mut() {
            *is_file = !is_dir;
        }
        key
    });
    Ok(rows)
}

pub(super) fn matches(
    root: &Path,
    query: &str,
    rows: &[(ExplorerItem, usize)],
) -> (Vec<ExplorerItem>, usize) {
    let query = query.replace(std::path::MAIN_SEPARATOR, "/");
    let pattern = nucleo::pattern::Pattern::parse(
        &query,
        nucleo::pattern::CaseMatching::Smart,
        nucleo::pattern::Normalization::Smart,
    );
    let mut matcher = nucleo::Matcher::new(nucleo::Config::DEFAULT.match_paths());
    let mut scratch = Vec::new();
    let mut first_match = None;
    let rows = ui_core::tree::filter_rows(
        rows.iter().map(|(item, depth)| (item, *depth)).collect(),
        |(path, _)| {
            let text = if query.contains('/') {
                Cow::Owned(
                    path.strip_prefix(root)
                        .unwrap_or(path)
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/"),
                )
            } else {
                path.file_name().unwrap_or_default().to_string_lossy()
            };
            if pattern
                .score(nucleo::Utf32Str::new(&text, &mut scratch), &mut matcher)
                .is_some()
            {
                first_match.get_or_insert_with(|| path.clone());
                true
            } else {
                false
            }
        },
    );
    let options: Vec<_> = rows.into_iter().map(|(item, _)| (*item).clone()).collect();
    let cursor = first_match
        .and_then(|path| options.iter().position(|(candidate, _)| *candidate == path))
        .unwrap_or(0);
    (options, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursive_search_excludes_metadata_but_keeps_useful_dotfiles() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        for name in [".git", ".hg", ".jj", ".github"] {
            std::fs::create_dir(dir.path().join(name))?;
            std::fs::write(dir.path().join(name).join("test.txt"), "")?;
        }
        std::fs::write(dir.path().join(".gitignore"), "")?;
        let rows = search_index(dir.path(), &Default::default(), || false)?;
        let paths: Vec<_> = rows
            .iter()
            .map(|((path, _), _)| path.strip_prefix(dir.path()).unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                Path::new(".github"),
                Path::new(".github/test.txt"),
                Path::new(".gitignore"),
            ]
        );
        Ok(())
    }

    #[test]
    fn recursive_search_orders_each_directory_like_the_browser() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        for name in ["b", "c", "b/z"] {
            std::fs::create_dir_all(dir.path().join(name))?;
        }
        for name in ["a.txt", "b/a.txt", "b/z/test.txt", "c/test.txt"] {
            std::fs::write(dir.path().join(name), "")?;
        }
        let rows = search_index(dir.path(), &Default::default(), || false)?;
        let paths: Vec<_> = rows
            .iter()
            .map(|((path, _), _)| path.strip_prefix(dir.path()).unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                "b",
                "b/z",
                "b/z/test.txt",
                "b/a.txt",
                "c",
                "c/test.txt",
                "a.txt"
            ]
            .map(Path::new)
        );
        Ok(())
    }

    #[test]
    fn exact_directory_queries_skip_fuzzy_matching_ancestors() {
        let root = Path::new("workspace");
        let fixture = root.join("sidebar-fixture-KRXB3w");
        let rows = [
            ((fixture.clone(), true), 0),
            ((fixture.join("sub"), true), 1),
        ];
        let (_, fuzzy_cursor) = matches(root, "sub", &rows);
        assert_eq!(fuzzy_cursor, 0);
        let (options, exact_cursor) = matches(root, "^sub$", &rows);
        assert_eq!(options[exact_cursor].0, fixture.join("sub"));
    }

    #[test]
    fn search_accepts_portable_and_native_path_separators() {
        let root = Path::new("workspace");
        let rows = [
            ((root.join("alpha"), true), 0),
            ((root.join("alpha").join("a_test.rs"), false), 1),
        ];
        for query in [
            "alpha/a_test.rs".to_owned(),
            format!("alpha{}a_test.rs", std::path::MAIN_SEPARATOR),
        ] {
            let (options, cursor) = matches(root, &query, &rows);
            assert_eq!(options.len(), 2, "query: {query}");
            assert_eq!(options[cursor].0, root.join("alpha").join("a_test.rs"));
        }
    }

    #[test]
    fn search_selects_the_first_match_with_its_parents_above_it() {
        let root = Path::new("workspace");
        let rows = [
            ((root.join("alpha"), true), 0),
            ((root.join("alpha/a_test.rs"), false), 1),
            ((root.join("zeta"), true), 0),
            ((root.join("zeta/test.rs"), false), 1),
        ];
        let (options, cursor) = matches(root, "test", &rows);
        assert_eq!(options.len(), 4);
        assert_eq!(cursor, 1);
        assert_eq!(options[cursor].0, root.join("alpha/a_test.rs"));
    }
}

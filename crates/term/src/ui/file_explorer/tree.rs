//! Cached workspace search with matching rows and their parent chains.

use super::{content, ExplorerData, ExplorerItem};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use view::Editor;

type ExplorerRows = Vec<(ExplorerItem, usize)>;

/// Immutable source revision. Normalize paths and discover parents once during
/// the background filesystem walk, rather than on every query keystroke.
struct SearchIndex {
    rows: ExplorerRows,
    paths: Vec<String>,
    name_starts: Vec<usize>,
    parents: Vec<Option<usize>>,
}

impl SearchIndex {
    fn new(root: &Path, rows: ExplorerRows) -> Self {
        let mut ancestors = Vec::new();
        let mut parents = Vec::with_capacity(rows.len());
        let mut paths = Vec::with_capacity(rows.len());
        let mut name_starts = Vec::with_capacity(rows.len());
        for (index, ((path, _), depth)) in rows.iter().enumerate() {
            ancestors.truncate(*depth);
            parents.push(ancestors.last().copied());
            ancestors.push(index);
            let text = path
                .strip_prefix(root)
                .unwrap_or(path)
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            name_starts.push(text.rfind('/').map_or(0, |separator| separator + 1));
            paths.push(text);
        }
        Self {
            rows,
            paths,
            name_starts,
            parents,
        }
    }

    pub(super) fn matches(
        &self,
        query: &str,
        cancelled: impl Fn() -> bool,
    ) -> (Vec<ExplorerItem>, usize) {
        let query = query.replace(std::path::MAIN_SEPARATOR, "/");
        let pattern = nucleo::pattern::Pattern::parse(
            &query,
            nucleo::pattern::CaseMatching::Smart,
            nucleo::pattern::Normalization::Smart,
        );
        let mut matcher = nucleo::Matcher::new(nucleo::Config::DEFAULT.match_paths());
        let mut scratch = Vec::new();
        let mut keep = vec![false; self.rows.len()];
        let mut first_match = None;
        let path_query = query.contains('/');
        for (index, path) in self.paths.iter().enumerate() {
            let text = if path_query {
                &**path
            } else {
                &path[self.name_starts[index]..]
            };
            if index % 256 == 0 && cancelled() {
                return (Vec::new(), 0);
            }
            if pattern
                .score(nucleo::Utf32Str::new(text, &mut scratch), &mut matcher)
                .is_some()
            {
                first_match.get_or_insert(index);
                keep[index] = true;
                let mut parent = self.parents[index];
                while let Some(index) = parent {
                    if keep[index] {
                        break;
                    }
                    keep[index] = true;
                    parent = self.parents[index];
                }
            }
        }
        let cursor = first_match.map_or(0, |first| {
            keep[..first].iter().filter(|keep| **keep).count()
        });
        let mut options = Vec::new();
        for (index, ((item, _), keep)) in self.rows.iter().zip(keep).enumerate() {
            if index % 256 == 0 && cancelled() {
                return (Vec::new(), 0);
            }
            if keep {
                options.push(item.clone());
            }
        }
        (options, cursor)
    }
}

#[derive(Clone, Default)]
pub struct ExplorerTree {
    pub expanded: ui_core::tree::Tree<PathBuf>,
    pub searching: bool,
    // Reuse one recursive listing across keystrokes. Background filesystem
    // refreshes replace it so subsequent queries see the same source revision.
    index: Arc<Mutex<Index>>,
}

#[derive(Default)]
enum Index {
    #[default]
    Empty,
    Ready(Arc<SearchIndex>),
    Failed,
}

impl ExplorerTree {
    pub fn clear_index(&self) {
        *self.index.lock().unwrap() = Index::Empty;
    }

    pub fn reset_index(&mut self) {
        self.index = Default::default();
    }

    pub fn search_failed(&self) -> bool {
        matches!(*self.index.lock().unwrap(), Index::Failed)
    }

    fn load_index(
        &self,
        root: &Path,
        config: &editor_config::FileExplorerConfig,
        cancel: &event::TaskHandle,
    ) -> std::io::Result<Arc<SearchIndex>> {
        if let Index::Ready(index) = &*self.index.lock().unwrap() {
            return Ok(index.clone());
        }
        let result = search_index(root, config, || cancel.is_canceled());
        let mut cached = self.index.lock().unwrap();
        if !cancel.is_canceled() {
            *cached = match &result {
                Ok(index) => Index::Ready(index.clone()),
                Err(_) => Index::Failed,
            };
        }
        result
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
        tree.clear_index();
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
        // Nonempty matching is handled by the background query source.
        (Vec::new(), 0)
    };
    Ok((options, data, cursor))
}

fn search_index(
    root: &Path,
    config: &editor_config::FileExplorerConfig,
    cancelled: impl Fn() -> bool,
) -> std::io::Result<Arc<SearchIndex>> {
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
    Ok(Arc::new(SearchIndex::new(root, rows)))
}

pub(super) fn query_async(
    editor: &Editor,
    query: &str,
    data: &ExplorerData,
    selected: Option<&ExplorerItem>,
    cancel: event::TaskHandle,
) -> futures_util::future::BoxFuture<
    'static,
    anyhow::Result<(Vec<ExplorerItem>, ExplorerData, usize)>,
> {
    let query = query.to_owned();
    let mut data = data.clone();
    let selected = selected.cloned();
    let tree = data.tree.as_mut().unwrap();
    tree.searching = true;
    if tree.search_failed() {
        tree.clear_index();
    }
    let config = editor.config().file_explorer.clone();
    Box::pin(async move {
        tokio::task::spawn_blocking(move || {
            let tree = data.tree.as_ref().unwrap();
            let index = tree.load_index(&data.root, &config, &cancel)?;
            let (options, first) = index.matches(&query, || cancel.is_canceled());
            let cursor = selected
                .and_then(|selected| options.iter().position(|item| *item == selected))
                .unwrap_or(first);
            Ok((options, data, cursor))
        })
        .await?
    })
}

/// Filesystem refreshes and query matching share the same index loader.
pub(super) fn refresh(
    query: &str,
    editor: &Editor,
    data: Arc<ExplorerData>,
    cancel: event::TaskHandle,
) -> futures_util::future::BoxFuture<'static, anyhow::Result<Vec<ExplorerItem>>> {
    if let Some(tree) = &data.tree {
        tree.clear_index();
        if !query.is_empty() {
            let work = query_async(editor, query, &data, None, cancel);
            return Box::pin(async move { Ok(work.await?.0) });
        }
    }
    let config = editor.config().file_explorer.clone();
    Box::pin(async move {
        Ok(tokio::task::spawn_blocking(move || {
            content(&data.root, data.tree.as_ref(), &config, || {
                cancel.is_canceled()
            })
        })
        .await??)
    })
}

#[cfg(test)]
fn matches(root: &Path, query: &str, rows: &[(ExplorerItem, usize)]) -> (Vec<ExplorerItem>, usize) {
    SearchIndex::new(root, rows.to_vec()).matches(query, || false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_matching_cancels_between_batches() {
        let root = Path::new("workspace");
        let rows = (0..10_000)
            .map(|i| ((root.join(format!("file{i}.rs")), false), 0))
            .collect();
        let index = SearchIndex::new(root, rows);
        let checks = std::cell::Cell::new(0);
        let (options, _) = index.matches("file", || {
            checks.set(checks.get() + 1);
            checks.get() == 2
        });
        assert!(options.is_empty());
        assert_eq!(checks.get(), 2);
    }

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
            .rows
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
            .rows
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

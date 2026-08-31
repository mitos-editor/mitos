//! Buffer search coordination without prompts or status messages.

use crate::features::buffer_search::Query;
use crate::Editor;
use editor_core::{
    movement::{Direction, Movement},
    search,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOutcome {
    Found { wrapped: bool },
    NotFound,
}

impl Editor {
    /// Search repeatedly, updating the primary selection or adding matches. Stop
    /// at the first missing match and center the final cursor once.
    pub fn search_buffer(
        &mut self,
        query: &Query,
        movement: Movement,
        direction: Direction,
        count: usize,
        wrap: bool,
    ) -> SearchOutcome {
        let config = self.config();
        let padding = config.scroll.padding;
        let handler = self.handlers.buffer_search.clone();
        let (view, doc) = current!(self);
        let mut outcome = SearchOutcome::NotFound;
        let mut wrapped = false;
        let mut changed = false;
        for _ in 0..count {
            let selection = doc.selection(view.id);
            let Some(found) = search::find_regex(
                doc.text().slice(..),
                selection.primary(),
                query.regex(),
                direction,
                wrap,
            ) else {
                outcome = SearchOutcome::NotFound;
                break;
            };
            let selection = match movement {
                Movement::Extend => selection.clone().push(found.range),
                Movement::Move => selection
                    .clone()
                    .replace(selection.primary_index(), found.range),
            };
            doc.set_selection(view.id, selection);
            changed = true;
            wrapped |= found.wrapped;
            outcome = SearchOutcome::Found { wrapped };
        }
        if changed {
            view.ensure_cursor_in_view_center(doc, padding);
        }
        if changed && config.statusline.shows_search_position() {
            let range = doc.selection(view.id).primary();
            handler.request(doc, view.id, query, range, config.search.match_limit());
        } else if let Some(state) = doc.search_state_mut(view.id) {
            state.clear_position();
        }
        outcome
    }
}

impl Editor {
    /// Replace the selected workspace match, or every occurrence in the supplied files.
    /// Plan and validate every document before editing. Changes remain unsaved and
    /// form one independent undo step per document, including hidden buffers.
    pub fn replace_workspace_matches(
        &mut self,
        search: &::search::Search,
        matches: &[&::search::Match],
        replacement: &str,
        replace_all: bool,
    ) -> anyhow::Result<usize> {
        use anyhow::{ensure, Context as _};
        use editor_core::Transaction;
        use std::collections::HashSet;
        use stdx::rope::RopeExt as _;

        let mut seen = HashSet::new();
        let mut plans = Vec::new();
        let mut count = 0;
        for matched in matches {
            let id = self
                .open(&matched.path, super::Action::Load)
                .with_context(|| format!("failed to open '{}'", matched.path.display()))?;
            if !seen.insert(id) {
                continue;
            }
            let doc = self.document(id).unwrap();
            let text = doc.text().slice(..);
            let target = if replace_all {
                None
            } else {
                ensure!(
                    matched.line_start <= matched.line_end
                        && matched.line_end < text.len_lines(stdx::rope::LINE_TYPE),
                    "the selected match no longer exists"
                );
                let start = text
                    .line_to_char_idx(matched.line_start)
                    .checked_add(matched.match_start_col);
                let end = text
                    .line_to_char_idx(matched.line_end)
                    .checked_add(matched.match_end_col);
                let (Some(start), Some(end)) = (start, end) else {
                    anyhow::bail!("the selected match no longer exists");
                };
                ensure!(
                    start <= end
                        && start
                            <= editor_core::line_ending::line_end_char_index(
                                &text,
                                matched.line_start
                            )
                        && end
                            <= editor_core::line_ending::line_end_char_index(
                                &text,
                                matched.line_end
                            ),
                    "the selected match no longer exists"
                );
                Some(start..end)
            };
            let changes = search.replacement_changes(text, replacement, target)?;
            ensure!(
                replace_all || !changes.is_empty(),
                "the selected text no longer matches the search pattern"
            );
            count += changes.len();
            if !changes.is_empty() {
                let transaction = Transaction::change(doc.text(), changes.into_iter());
                doc.validate_changes(transaction.changes())?;
                plans.push((id, doc.version(), transaction));
            }
            if !replace_all {
                break;
            }
        }
        for (id, version, transaction) in plans {
            self.apply_document_changes(id, version, transaction.changes())?;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod replacement_tests {
    use crate::editor::{tests::editor, Action};
    use editor_core::Rope;

    fn matched(path: &std::path::Path, start: usize, end: usize) -> ::search::Match {
        ::search::Match {
            path: path.into(),
            line_start: 0,
            line_end: 0,
            match_start_col: start,
            match_end_col: end,
        }
    }

    #[tokio::test]
    async fn replacements_use_unsaved_hidden_buffers_and_independent_history() -> anyhow::Result<()>
    {
        let (mut editor, _store) = editor("#[|]#\n");
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("file.txt");
        std::fs::write(&path, "disk\n")?;
        let id = editor.open(&path, Action::Replace)?;
        editor.enter_insert_mode();
        editor.insert_char('α')?;
        editor.open(&path, Action::VerticalSplit)?;
        let hidden = editor.new_file(Action::Replace);
        let hidden_text = editor.document(hidden).unwrap().text().clone();
        let focus = editor.tree.focus();
        let search = ::search::Search::new("α", true)?;
        assert_eq!(
            editor.replace_workspace_matches(&search, &[&matched(&path, 0, 1)], "long", false)?,
            1
        );
        assert_eq!(editor.document(id).unwrap().text(), "longdisk\n");
        assert_eq!(editor.document(hidden).unwrap().text(), &hidden_text);
        assert_eq!(editor.tree.focus(), focus);
        assert_eq!(std::fs::read_to_string(&path)?, "disk\n");
        editor.open(&path, Action::Replace)?;
        assert!(editor.change_history(buffer::HistoryAction::Undo));
        assert_eq!(editor.document(id).unwrap().text(), "αdisk\n");
        assert!(editor.change_history(buffer::HistoryAction::Undo));
        assert_eq!(editor.document(id).unwrap().text(), "disk\n");
        Ok(())
    }

    #[tokio::test]
    async fn replace_all_deduplicates_files_and_preserves_other_buffers() -> anyhow::Result<()> {
        let (mut editor, _store) = editor("#[|]#\n");
        let dir = tempfile::tempdir()?;
        let paths: Vec<_> = ["one", "two", "excluded"]
            .map(|name| dir.path().join(name))
            .into();
        for path in &paths {
            std::fs::write(path, "foo foo\n")?;
        }
        let a = matched(&paths[0], 0, 3);
        let b = matched(&paths[1], 0, 3);
        let alias = matched(&dir.path().join("sub/../one"), 0, 3);
        let search = ::search::Search::new("foo", true)?;
        assert_eq!(
            editor.replace_workspace_matches(&search, &[&a, &a, &alias, &b], "bar", true)?,
            4
        );
        for path in &paths[..2] {
            let id = editor.open(path, Action::Load)?;
            assert_eq!(editor.document(id).unwrap().text(), "bar bar\n");
            assert_eq!(std::fs::read_to_string(path)?, "foo foo\n");
        }
        assert_eq!(std::fs::read_to_string(&paths[2])?, "foo foo\n");
        Ok(())
    }

    #[tokio::test]
    async fn invalid_later_document_prevents_partial_replacement() -> anyhow::Result<()> {
        let (mut editor, _store) = editor("#[|]#\n");
        let dir = tempfile::tempdir()?;
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        std::fs::write(&first, "foo\n")?;
        std::fs::write(&second, "foo\n")?;
        let id = editor.open(&first, Action::Load)?;
        let mut doc = crate::Document::from(
            Rope::from("foo\n"),
            None,
            editor.config.clone(),
            editor.syn_loader.load_full(),
        );
        doc.set_path(Some(&second));
        let doc = doc.with_readonly_name("source".into());
        editor.new_file_from_document(Action::Replace, doc);
        let search = ::search::Search::new("foo", true)?;
        assert!(editor
            .replace_workspace_matches(
                &search,
                &[&matched(&first, 0, 3), &matched(&second, 0, 3)],
                "bar",
                true
            )
            .is_err());
        assert_eq!(editor.document(id).unwrap().text(), "foo\n");
        assert!(editor
            .replace_workspace_matches(&search, &[&matched(&first, 1, 4)], "bar", false)
            .is_err());
        assert_eq!(editor.document(id).unwrap().text(), "foo\n");
        Ok(())
    }
}

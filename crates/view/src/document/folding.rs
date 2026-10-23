//! Closed folds belong to a document/view pair, never to the buffer or syntax tree.

use super::Document;
use crate::ViewId;
use editor_core::{line_ending::line_end_char_index, Assoc, ChangeSet, RopeSlice, Selection};
use stdx::rope::RopeExt as _;
use text_layout::text_annotations::Conceal;

#[derive(Debug, Clone, Default)]
pub(super) struct Folds {
    /// Sorted outermost first. Closed descendants survive reopening a parent.
    closed: Vec<Conceal>,
    visible: Vec<Conceal>,
}

impl Folds {
    fn rebuild(&mut self) {
        self.closed
            .sort_unstable_by_key(|c| (c.range.start, std::cmp::Reverse(c.range.end)));
        self.closed.dedup();
        // Keep a laminar set even for crossing injection/selection ranges. Sorting
        // and a stack make folding an entire large file O(n log n).
        let mut ends = Vec::new();
        self.closed.retain(|fold| {
            while ends.last().is_some_and(|end| *end < fold.range.start) {
                ends.pop();
            }
            if ends.last().is_some_and(|end| *end < fold.range.end) {
                return false;
            }
            ends.push(fold.range.end);
            true
        });
        self.visible.clear();
        for fold in &self.closed {
            if self
                .visible
                .last()
                .is_none_or(|parent| parent.range.end < fold.range.start)
            {
                self.visible.push(fold.clone());
            }
        }
    }

    pub(super) fn reveal(&mut self, selection: &Selection, text: RopeSlice) {
        let len = self.closed.len();
        self.closed.retain(|fold| {
            !selection.iter().any(|r| {
                let cursor = r.cursor(text);
                fold.range.start < cursor && cursor <= fold.range.end
            })
        });
        if len != self.closed.len() {
            self.rebuild();
        }
    }

    pub(super) fn apply_changes(&mut self, changes: &ChangeSet) {
        self.closed.retain_mut(|fold| {
            // An edit touching concealed text opens it in every view. Changes
            // elsewhere track the same text, including undo and external edits.
            if changes
                .changes_iter()
                .any(|(start, end, _)| start <= fold.range.end && end >= fold.range.start)
            {
                return false;
            }
            fold.range.start = changes.map_pos(fold.range.start, Assoc::After);
            fold.range.end = changes.map_pos(fold.range.end, Assoc::Before);
            fold.range.start < fold.range.end
        });
        self.rebuild();
    }
}

/// A folding operation applied to each cursor in the current view.
#[derive(Debug, Clone, Copy)]
pub enum FoldAction {
    /// Close the innermost open syntax region containing each cursor.
    Close,
    /// Open the outermost closed region containing each cursor's line.
    Open,
    Toggle,
    /// Fold the lines covered by each selection, keeping its first line visible.
    Selection,
    /// Close all syntax regions, retaining nested closed state.
    CloseAll,
    OpenAll,
}

/// Convert a syntax or selection range to a whole-line conceal. The final line
/// ending is kept, so the next document line starts a new display row.
fn conceal(text: RopeSlice, start_line: usize, end_line: usize) -> Option<Conceal> {
    (start_line < end_line)
        .then(|| Conceal {
            range: line_end_char_index(&text, start_line)..line_end_char_index(&text, end_line),
        })
        .filter(|c| c.range.start < c.range.end)
}

impl Document {
    pub fn folded_ranges(&self, view: ViewId) -> &[Conceal] {
        self.view_states
            .get(&view)
            .map_or(&[], |state| &state.folds.visible)
    }

    /// Change folding without modifying document text or undo history.
    pub fn fold(&mut self, view: ViewId, action: FoldAction) {
        if matches!(
            action,
            FoldAction::Close | FoldAction::Toggle | FoldAction::CloseAll
        ) {
            self.finish_syntax_initialization();
        }
        let text = self.buffer.text().slice(..);
        let selection = self.selection(view).clone();
        let candidates: Vec<_> = if matches!(
            action,
            FoldAction::Close | FoldAction::Toggle | FoldAction::CloseAll
        ) {
            self.syntax
                .as_ref()
                .map(|syntax| {
                    if matches!(action, FoldAction::CloseAll) {
                        syntax.fold_ranges(text, &self.syntax_loader, ..)
                    } else {
                        selection
                            .iter()
                            .flat_map(|range| {
                                let line = text.char_to_line_idx(range.cursor(text));
                                let bytes = text.line_to_byte_idx(line, stdx::rope::LINE_TYPE)
                                    ..text.line_to_byte_idx(line + 1, stdx::rope::LINE_TYPE);
                                syntax.fold_ranges(
                                    text,
                                    &self.syntax_loader,
                                    bytes.start as u32..bytes.end as u32,
                                )
                            })
                            .collect()
                    }
                })
                .unwrap_or_default()
                .into_iter()
                .filter_map(|range| {
                    if range.is_empty() {
                        return None;
                    }
                    conceal(
                        text,
                        text.char_to_line_idx(range.start),
                        text.char_to_line_idx(range.end - 1),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let state = self
            .view_states
            .get_mut(&view)
            .expect("attached document view");
        match action {
            FoldAction::OpenAll => state.folds = Folds::default(),
            FoldAction::CloseAll => {
                state.folds.closed.extend(candidates);
                state.folds.rebuild();
            }
            _ => {
                // Resolve all cursors against one snapshot. Two cursors on the
                // same fold must toggle it once, rather than undo each other.
                let mut close = Vec::new();
                let mut open = Vec::new();
                for range in &selection {
                    let cursor = range.cursor(text);
                    let line = text.char_to_line_idx(cursor);
                    let closed = state
                        .folds
                        .visible
                        .iter()
                        .find(|c| {
                            text.char_to_line_idx(c.range.start) <= line
                                && line <= text.char_to_line_idx(c.range.end)
                        })
                        .cloned();
                    if matches!(action, FoldAction::Open | FoldAction::Toggle)
                        && let Some(closed) = &closed
                    {
                        open.push(closed.clone());
                        continue;
                    }
                    match action {
                        FoldAction::Selection => {
                            let (start, end) = range.line_range(text);
                            if let Some(fold) = conceal(text, start, end) {
                                close.push(fold);
                            }
                        }
                        FoldAction::Close | FoldAction::Toggle => {
                            let candidate = candidates
                                .iter()
                                .filter(|candidate| {
                                    let contains_cursor =
                                        text.char_to_line_idx(candidate.range.start) <= line
                                            && line <= text.char_to_line_idx(candidate.range.end);
                                    let encloses_closed = closed.as_ref().is_none_or(|closed| {
                                        candidate.range.start <= closed.range.start
                                            && candidate.range.end >= closed.range.end
                                    });
                                    contains_cursor
                                        && encloses_closed
                                        && !state.folds.closed.contains(candidate)
                                })
                                .min_by_key(|candidate| {
                                    candidate.range.end - candidate.range.start
                                });
                            if let Some(candidate) = candidate {
                                close.push(candidate.clone());
                            }
                        }
                        _ => {}
                    }
                }
                state.folds.closed.retain(|fold| !open.contains(fold));
                state.folds.closed.extend(close);
                state.folds.rebuild();
            }
        }
        // Closing a region moves any hidden cursors to its visible header.
        // Retain other selections and the primary range in multi-cursor mode.
        let selection = selection.transform(|range| {
            let cursor = range.cursor(text);
            if let Some(fold) = state
                .folds
                .visible
                .iter()
                .find(|c| c.range.start < cursor && cursor <= c.range.end)
            {
                let header = text.line_to_char_idx(text.char_to_line_idx(fold.range.start));
                editor_core::Range::point(header)
            } else {
                range
            }
        });
        // A viewport anchored inside a newly closed fold must use its header too.
        if let Some(fold) = state
            .folds
            .visible
            .iter()
            .find(|c| c.range.contains(&state.position.anchor))
        {
            state.position.anchor = text.line_to_char_idx(text.char_to_line_idx(fold.range.start));
            state.position.vertical_offset = 0;
        }
        self.set_selection(view, selection);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::{DocumentDidChange, SelectionDidChange},
        graphics::Rect,
        View,
    };
    use arc_swap::ArcSwap;
    use editor_core::{Range, Rope, Transaction};
    use std::sync::Arc;

    fn fixture(text: &str, language: Option<&str>) -> (Document, View, View) {
        event::register_event::<DocumentDidChange>();
        event::register_event::<SelectionDidChange>();
        let loader = Arc::new(language::loader::default_lang_loader(
            loader::syntax::Resources::default(),
        ));
        let mut doc = Document::from(
            Rope::from_str(text),
            None,
            Arc::new(ArcSwap::from_pointee(editor_config::Config::default())),
            loader.clone(),
        );
        if let Some(name) = language {
            let language = loader.language_for_name(name).unwrap();
            doc.set_language(Some(loader.language(language).config().clone()), &loader);
            assert!(doc.syntax.is_some(), "grammar required for {name}");
        }
        let mut ids = slotmap::SlotMap::<ViewId, ()>::with_key();
        let mut make_view = || {
            let mut view = View::new(doc.id(), Default::default(), event::Redraw::default());
            view.id = ids.insert(());
            view.area = Rect::new(0, 0, 80, 10);
            view
        };
        let first = make_view();
        let second = make_view();
        doc.ensure_view_init(first.id);
        doc.ensure_view_init(second.id);
        (doc, first, second)
    }

    #[tokio::test]
    async fn syntax_folds_nest_and_reopening_a_parent_keeps_closed_children() {
        let (mut doc, view, other) = fixture(
            "fn main() {\n    if true {\n        work();\n    }\n}\nnext();",
            Some("rust"),
        );
        let text = doc.text().clone();
        let inner = text.line_to_char_idx(1);
        doc.set_selection(view.id, Selection::point(inner));
        doc.fold(view.id, FoldAction::Close);
        assert_eq!(
            doc.folded_ranges(view.id),
            &[conceal(text.slice(..), 1, 3).unwrap()]
        );
        doc.fold(view.id, FoldAction::Close);
        assert_eq!(
            doc.folded_ranges(view.id),
            &[conceal(text.slice(..), 0, 4).unwrap()]
        );
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            0
        );
        assert!(doc.folded_ranges(other.id).is_empty());
        doc.fold(view.id, FoldAction::Open);
        assert_eq!(
            doc.folded_ranges(view.id),
            &[conceal(text.slice(..), 1, 3).unwrap()]
        );
        assert_eq!(doc.text(), &text);
        assert!(!doc.is_modified());
        doc.fold(view.id, FoldAction::OpenAll);
        assert!(doc.folded_ranges(view.id).is_empty());
        doc.fold(view.id, FoldAction::CloseAll);
        assert_eq!(
            doc.folded_ranges(view.id),
            &[conceal(text.slice(..), 0, 4).unwrap()]
        );
        doc.fold(view.id, FoldAction::Open);
        assert_eq!(
            doc.folded_ranges(view.id),
            &[conceal(text.slice(..), 1, 3).unwrap()]
        );
    }

    #[tokio::test]
    async fn manual_folds_support_plain_text_and_reveal_jump_destinations_per_view() {
        let (mut doc, view, other) = fixture("header\nhidden\nlast\nnext", None);
        let selected = Selection::single(0, doc.text().line_to_char_idx(3));
        for id in [view.id, other.id] {
            doc.set_selection(id, selected.clone());
            doc.fold(id, FoldAction::Selection);
        }
        let fold = conceal(doc.text().slice(..), 0, 2).unwrap();
        assert_eq!(doc.folded_ranges(view.id), std::slice::from_ref(&fold));
        assert_eq!(
            view.pos_at_visual_coords(&doc, 1, 0, true),
            Some(doc.text().line_to_char_idx(3))
        );
        doc.set_selection(view.id, Selection::point(doc.text().line_to_char_idx(1)));
        assert!(doc.folded_ranges(view.id).is_empty());
        assert_eq!(doc.folded_ranges(other.id), &[fold]);
        doc.remove_view(other.id);
        doc.ensure_view_init(other.id);
        assert!(doc.folded_ranges(other.id).is_empty());
    }

    #[tokio::test]
    async fn edits_map_unaffected_folds_and_open_touched_folds_in_all_views() {
        for newline in ["\n", "\r\n"] {
            let (mut doc, view, other) = fixture(
                &format!("before{newline}header{newline}hidden{newline}last{newline}after"),
                None,
            );
            for id in [view.id, other.id] {
                doc.set_selection(
                    id,
                    Selection::single(
                        doc.text().line_to_char_idx(1),
                        doc.text().line_to_char_idx(4),
                    ),
                );
                doc.fold(id, FoldAction::Selection);
            }
            let insert = Transaction::change(
                doc.text(),
                [(0, 0, Some(format!("extra{newline}").into()))].into_iter(),
            );
            doc.apply(insert.changes()).unwrap();
            doc.commit_changes();
            for id in [view.id, other.id] {
                assert_eq!(
                    doc.folded_ranges(id),
                    &[conceal(doc.text().slice(..), 2, 4).unwrap()]
                );
            }
            assert!(doc.undo());
            assert_eq!(
                doc.folded_ranges(view.id),
                &[conceal(doc.text().slice(..), 1, 3).unwrap()]
            );
            assert!(doc.redo());
            let hidden = doc.text().line_to_char_idx(3);
            let edit = Transaction::change(
                doc.text(),
                [(hidden, hidden + 1, Some("X".into()))].into_iter(),
            );
            doc.apply(edit.changes()).unwrap();
            assert!(doc.folded_ranges(view.id).is_empty());
            assert!(doc.folded_ranges(other.id).is_empty());
        }
    }

    #[tokio::test]
    async fn multiple_cursors_toggle_a_shared_fold_once_and_keep_the_primary_range() {
        let (mut doc, view, _) = fixture("{\n  \"value\": [\n    1, 2\n  ]\n}\n", Some("json"));
        doc.set_selection(
            view.id,
            Selection::new(vec![Range::point(0), Range::point(1)].into(), 1),
        );
        doc.fold(view.id, FoldAction::Toggle);
        assert_eq!(doc.folded_ranges(view.id).len(), 1);
        assert_eq!(doc.selection(view.id).primary_index(), 1);
        doc.fold(view.id, FoldAction::Toggle);
        assert!(doc.folded_ranges(view.id).is_empty());
    }

    #[tokio::test]
    async fn completing_pending_syntax_preserves_manual_folds_until_the_language_changes() {
        let (mut doc, view, _) = fixture("{\n  \"key\": 1\n}\n", None);
        let loader = doc.syntax_loader.clone();
        doc.set_path(Some(std::path::Path::new("pending.json")));
        doc.detect_language(&loader);
        assert!(doc.is_syntax_pending());
        doc.set_selection(view.id, Selection::single(0, doc.text().len_chars()));
        doc.fold(view.id, FoldAction::Selection);
        let before = doc.folded_ranges(view.id).to_vec();
        assert_eq!(before.len(), 1);
        assert!(doc.finish_syntax_initialization());
        assert_eq!(doc.folded_ranges(view.id), before);
        doc.set_language(None, &loader);
        assert!(doc.folded_ranges(view.id).is_empty());
    }

    #[tokio::test]
    async fn window_motion_and_scrolling_count_visible_rows_through_a_large_fold() {
        use crate::editing::movement::{goto_window, scroll, ScrollCursor};
        use editor_core::movement::{Direction, Movement};
        let (mut doc, mut view, _) =
            fixture(&format!("header\n{}next", "hidden\n".repeat(200)), None);
        view.area.height = 3; // Two text rows and the per-view statusline.
        let next = doc.text().line_to_char_idx(201);
        doc.set_selection(view.id, Selection::single(0, next));
        doc.fold(view.id, FoldAction::Selection);
        let layout = view.layout(&doc, None);
        assert_eq!(layout.last_visual_line(), 1);
        assert_eq!(layout.last_document_line(), 201);
        assert_eq!(layout.visible_byte_ranges().len(), 2);
        drop(layout);
        goto_window(&mut doc, &view, crate::Align::Bottom, 1, Movement::Move, 0);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            next
        );
        scroll(
            &mut doc,
            &view,
            1,
            Direction::Forward,
            ScrollCursor::KeepVisible,
            Movement::Move,
            0,
        );
        assert_eq!(doc.view_offset(view.id).anchor, next);
        assert_eq!(view.layout(&doc, None).last_visual_line(), 0);
    }

    #[test]
    fn crossing_regions_are_ignored_and_nested_regions_remain_available() {
        let mut folds = Folds {
            closed: [1..10, 4..7, 6..15, 16..20, 20..25]
                .map(|range| Conceal { range })
                .to_vec(),
            visible: Vec::new(),
        };
        folds.rebuild();
        assert_eq!(
            folds
                .closed
                .iter()
                .map(|c| c.range.clone())
                .collect::<Vec<_>>(),
            [1..10, 4..7, 16..20]
        );
        assert_eq!(
            folds
                .visible
                .iter()
                .map(|c| c.range.clone())
                .collect::<Vec<_>>(),
            [1..10, 16..20]
        );
    }
}

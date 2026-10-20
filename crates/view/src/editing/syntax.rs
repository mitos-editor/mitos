//! Shared structural selection, matching, and syntax navigation operations.

use crate::{Document, View, ViewId};
use editor_core::{
    movement::{Direction, Movement},
    Range,
};
use language::object;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SelectionAction {
    Expand,
    Shrink,
    NextSibling,
    PreviousSibling,
    AllSiblings,
    AllChildren,
}

/// Change structural selections, retaining expansion history only for expand/shrink.
/// Returns whether the view is bound and syntax or saved expansion history was available.
pub fn select(doc: &mut Document, view: ViewId, action: SelectionAction) -> bool {
    if doc.try_selection(view).is_none() {
        return false;
    }
    if action == SelectionAction::Shrink {
        let mut history = doc.take_expansion_history(view);
        if let Some(selection) = history.pop() {
            doc.set_expanded_selection(view, selection, history);
            return true;
        }
    }
    let Some(syntax) = doc.syntax() else {
        return false;
    };
    let text = doc.text().slice(..);
    let current = doc.selection(view).clone();
    let next = match action {
        SelectionAction::Expand => object::expand_selection(syntax, text, current.clone()),
        SelectionAction::Shrink => object::shrink_selection(syntax, text, current.clone()),
        SelectionAction::NextSibling => object::select_next_sibling(syntax, text, current.clone()),
        SelectionAction::PreviousSibling => {
            object::select_prev_sibling(syntax, text, current.clone())
        }
        SelectionAction::AllSiblings => object::select_all_siblings(syntax, text, current.clone()),
        SelectionAction::AllChildren => object::select_all_children(syntax, text, current.clone()),
    };
    if current != next {
        if action == SelectionAction::Expand {
            let mut history = doc.take_expansion_history(view);
            history.push(current);
            doc.set_expanded_selection(view, next, history);
        } else {
            doc.set_selection(view, next);
        }
    }
    true
}

pub fn move_parent_boundary(
    doc: &mut Document,
    view: ViewId,
    direction: Direction,
    movement: Movement,
) -> bool {
    let Some(syntax) = doc.syntax() else {
        return false;
    };
    let selection = language::movement::move_parent_node_end(
        syntax,
        doc.text().slice(..),
        doc.selection(view).clone(),
        direction,
        movement,
    );
    doc.set_selection(view, selection);
    true
}

/// Match syntax brackets when available, otherwise use plain-text bracket matching.
pub fn match_brackets(doc: &mut Document, view: ViewId, movement: Movement) {
    let text = doc.text().slice(..);
    let selection = doc.selection(view).clone().transform(|range| {
        let pos = range.cursor(text);
        let matched = match doc.syntax() {
            Some(syntax) => {
                language::match_brackets::find_matching_bracket_fuzzy(syntax, text, pos)
            }
            None => editor_core::match_brackets::find_matching_bracket_plaintext(text, pos),
        };
        matched.map_or(range, |pos| {
            range.put_cursor(text, pos, movement == Movement::Extend)
        })
    });
    doc.set_selection(view, selection);
}

/// Navigate to a query-backed object and save the originating jump.
/// Returns false when the document has no syntax tree.
pub fn goto_object(
    doc: &mut Document,
    view: &mut View,
    object: &str,
    direction: Direction,
    count: usize,
    movement: Movement,
) -> bool {
    let Some(syntax) = doc.syntax() else {
        return false;
    };
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id).clone().transform(|range| {
        let next = language::movement::goto_treesitter_object(
            text,
            range,
            object,
            direction,
            syntax,
            doc.syntax_loader(),
            count,
        );
        if movement == Movement::Extend {
            let head = if next.head < range.anchor {
                next.anchor
            } else {
                next.head
            };
            Range::new(range.anchor, head)
        } else {
            next.with_direction(direction)
        }
    });
    super::commit(doc, view);
    let jump = (doc.id(), doc.selection(view.id).clone());
    view.push_jump(doc, jump);
    doc.set_selection(view.id, selection);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_swap::ArcSwap;
    use editor_config::Config;
    use editor_core::{Rope, Selection, Transaction};
    use language::syntax;
    use std::sync::Arc;

    fn document(text: &str, language: &str) -> Document {
        event::register_event::<crate::events::SelectionDidChange>();
        event::register_event::<crate::events::DocumentDidChange>();
        let loader = Arc::new(syntax::Loader::new(
            toml::from_str(&format!("[[language]]\nname = '{language}'\nscope = 'source.{language}'\nfile-types = []")).unwrap(),
            loader::syntax::Resources::default(),
        ).unwrap());
        let mut doc = Document::from(
            Rope::from_str(text),
            None,
            Arc::new(ArcSwap::from_pointee(Config::default())),
            loader.clone(),
        );
        let language = loader.language_for_name(language).unwrap();
        doc.set_language(Some(loader.language(language).config().clone()), &loader);
        doc
    }

    fn view_ids() -> (ViewId, ViewId) {
        let mut ids = slotmap::SlotMap::<ViewId, ()>::with_key();
        (ids.insert(()), ids.insert(()))
    }

    fn expand_number_and_array(doc: &mut Document, view: ViewId) {
        doc.set_selection(view, Selection::point(1));
        assert!(select(doc, view, SelectionAction::Expand));
        assert!(select(doc, view, SelectionAction::Expand));
    }

    #[tokio::test]
    async fn expansion_history_belongs_to_each_document_and_split() {
        let (first, second) = view_ids();
        let mut a = document("[1,2]", "json");
        let mut b = document("[3,4]", "json");
        a.set_selection(first, Selection::point(1));
        a.set_selection(second, Selection::point(3));
        b.set_selection(first, Selection::point(3));
        let a_first = a.selection(first).clone();
        let a_second = a.selection(second).clone();
        let b_first = b.selection(first).clone();
        select(&mut a, first, SelectionAction::Expand);
        select(&mut b, first, SelectionAction::Expand);
        select(&mut a, second, SelectionAction::Expand);
        select(&mut a, first, SelectionAction::Shrink);
        select(&mut b, first, SelectionAction::Shrink);
        select(&mut a, second, SelectionAction::Shrink);
        assert_eq!(*a.selection(first), a_first);
        assert_eq!(*a.selection(second), a_second);
        assert_eq!(*b.selection(first), b_first);
    }

    #[tokio::test]
    async fn edits_discard_expansions_for_every_split() {
        let (first, second) = view_ids();
        let mut doc = document("[10,20]", "json");
        expand_number_and_array(&mut doc, first);
        expand_number_and_array(&mut doc, second);
        let edit = Transaction::change(doc.text(), [(2, 2, Some("1".into()))].into_iter());
        doc.apply(edit.changes()).unwrap();
        for view in [first, second] {
            select(&mut doc, view, SelectionAction::Shrink);
            assert_eq!(
                doc.selection(view).primary().fragment(doc.text().slice(..)),
                "["
            );
        }
    }

    #[tokio::test]
    async fn unrelated_selections_and_language_changes_discard_expansions() {
        let (view, _) = view_ids();
        let mut doc = document("[10,20]", "json");
        expand_number_and_array(&mut doc, view);
        doc.set_selection(view, Selection::point(5));
        doc.set_selection(view, Selection::single(0, 7));
        select(&mut doc, view, SelectionAction::Shrink);
        assert_eq!(
            doc.selection(view).primary().fragment(doc.text().slice(..)),
            "["
        );
        select(&mut doc, view, SelectionAction::Expand);
        let selection = doc.selection(view).clone();
        doc.set_language(None, &Arc::new(syntax::Loader::default()));
        assert!(!select(&mut doc, view, SelectionAction::Shrink));
        assert_eq!(*doc.selection(view), selection);
    }

    #[tokio::test]
    async fn removing_a_view_discards_its_expansion_history() {
        let (view, _) = view_ids();
        let mut doc = document("[10,20]", "json");
        expand_number_and_array(&mut doc, view);
        doc.remove_view(view);
        assert!(!select(&mut doc, view, SelectionAction::Shrink));
        assert!(doc.try_selection(view).is_none());
        doc.ensure_view_init(view);
        doc.set_selection(view, Selection::single(0, 7));
        select(&mut doc, view, SelectionAction::Shrink);
        assert_eq!(
            doc.selection(view).primary().fragment(doc.text().slice(..)),
            "["
        );
    }

    #[tokio::test]
    async fn expansions_restore_all_ranges_and_preserve_the_primary_selection() {
        let (view, _) = view_ids();
        let mut doc = document("[10,20]", "json");
        doc.set_selection(
            view,
            Selection::new(vec![Range::point(1), Range::point(4)].into(), 1),
        );
        let original = doc.selection(view).clone();
        select(&mut doc, view, SelectionAction::Expand);
        let fragments: Vec<_> = doc
            .selection(view)
            .iter()
            .map(|range| range.fragment(doc.text().slice(..)).into_owned())
            .collect();
        assert_eq!(fragments, ["10", "20"]);
        assert_eq!(doc.selection(view).primary_index(), 1);
        select(&mut doc, view, SelectionAction::Shrink);
        assert_eq!(*doc.selection(view), original);
    }

    #[tokio::test]
    async fn bracket_matching_works_without_syntax_and_extends_over_unicode() {
        let (view, _) = view_ids();
        let mut doc = document("(é)", "json");
        doc.set_language(None, &Arc::new(syntax::Loader::default()));
        doc.set_selection(view, Selection::point(0));
        match_brackets(&mut doc, view, Movement::Extend);
        assert_eq!(
            doc.selection(view).primary().fragment(doc.text().slice(..)),
            "(é)"
        );
        assert_eq!(
            doc.selection(view).primary().cursor(doc.text().slice(..)),
            2
        );
    }

    #[tokio::test]
    async fn syntax_navigation_records_the_origin_before_selecting_the_destination() {
        let mut doc = document("fn a() {}\nfn b() {}\n", "rust");
        let mut view = View::new(doc.id(), Default::default(), event::Redraw::default());
        doc.ensure_view_init(view.id);
        let original = doc.selection(view.id).clone();
        assert!(goto_object(
            &mut doc,
            &mut view,
            "function",
            Direction::Forward,
            1,
            Movement::Move
        ));
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .fragment(doc.text().slice(..)),
            "fn b() {}"
        );
        assert!(view
            .jumps
            .iter()
            .any(|(id, selection)| *id == doc.id() && *selection == original));
    }
}

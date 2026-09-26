//! Editing operations over documents and their per-view selections.

use editor_core::{line_ending::normalize_line_endings, Range, Tendril, Transaction};

use crate::{Document, View};

/// Replace every selection with the same literal text and commit pending edits to history.
///
/// Normalizes replacement line endings to the document's convention and selects
/// each inserted range, preserving selection direction and the primary selection.
/// Positions are character indices. The view must display this document and have
/// an initialized selection. Mode changes, scrolling, prompts, and repeat recording
/// belong to the caller.
pub fn replace_selections(doc: &mut Document, view: &mut View, replacement: &str) {
    let replacement = Tendril::from(normalize_line_endings(replacement, doc.line_ending).as_ref());
    let len = replacement.chars().count();
    let transaction =
        Transaction::change_by_and_with_selection(doc.text(), doc.selection(view.id), |range| {
            let selection =
                Range::new(range.from(), range.from() + len).with_direction(range.direction());
            (
                (range.from(), range.to(), Some(replacement.clone())),
                Some(selection),
            )
        });
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::ArcSwap;
    use editor_core::{syntax, test, LineEnding, Rope};

    use super::*;
    use crate::config::Config;

    fn document_and_view(input: &str) -> (Document, View) {
        let (text, selection) = test::print(input);
        let mut doc = Document::from(
            Rope::from(text),
            None,
            Arc::new(ArcSwap::from_pointee(Config::default())),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        let view = View::new(doc.id(), Default::default());
        doc.ensure_view_init(view.id);
        doc.set_selection(view.id, selection);
        (doc, view)
    }

    #[test]
    fn replacement_preserves_selections_and_undo_without_a_frontend() {
        for (input, replacement, line_ending, expected) in [
            (
                "#(e\u{301}|)# #[|界]#\n",
                "é界$1(",
                LineEnding::LF,
                "#(é界$1(|)# #[|é界$1(]#\n",
            ),
            (
                "#[foo|]# #(bar|)#\r\n",
                "one\ntwo\rthree\r\nfour",
                LineEnding::Crlf,
                "#[one\r\ntwo\r\nthree\r\nfour|]# #(one\r\ntwo\r\nthree\r\nfour|)#\r\n",
            ),
            ("foo#[|]#", "x", LineEnding::LF, "foo#[x|]#"),
            ("#[abc|]#\n", "", LineEnding::LF, "#[\n|]#"),
            ("#[|]#", "界", LineEnding::LF, "#[界|]#"),
        ] {
            let (mut doc, mut view) = document_and_view(input);
            doc.line_ending = line_ending;
            let original_text = doc.text().clone();
            let original_selection = doc.selection(view.id).clone();
            let version = doc.version();

            replace_selections(&mut doc, &mut view, replacement);

            assert_eq!(
                test::plain(doc.text().slice(..), doc.selection(view.id)),
                expected
            );
            assert_eq!(doc.version(), version + 1);
            assert!(doc.undo(&mut view));
            assert_eq!(doc.text(), &original_text);
            assert_eq!(doc.selection(view.id), &original_selection);
            assert!(!doc.undo(&mut view));
            assert!(doc.redo(&mut view));
            assert_eq!(
                test::plain(doc.text().slice(..), doc.selection(view.id)),
                expected
            );
        }
    }

    #[test]
    fn successive_replacements_have_separate_history_checkpoints() {
        let (mut doc, mut view) = document_and_view("#[original|]#\n");

        replace_selections(&mut doc, &mut view, "first");
        replace_selections(&mut doc, &mut view, "second");

        for expected in ["#[first|]#\n", "#[original|]#\n"] {
            assert!(doc.undo(&mut view));
            assert_eq!(
                test::plain(doc.text().slice(..), doc.selection(view.id)),
                expected
            );
        }
        for expected in ["#[first|]#\n", "#[second|]#\n"] {
            assert!(doc.redo(&mut view));
            assert_eq!(
                test::plain(doc.text().slice(..), doc.selection(view.id)),
                expected
            );
        }
    }
}

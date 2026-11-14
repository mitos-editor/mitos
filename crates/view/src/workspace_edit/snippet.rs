//! Prepare one snapshot-bound transaction from ordinary and snippet text edits.

use editor_core::{Selection, Transaction};
use lsp_client::{lsp, OffsetEncoding};
use snippets::{RenderedSnippet, Snippet};

use super::ApplyEditErrorKind;
use crate::{
    document::Mode, language_servers::edits::generate_transaction_from_edits, Document, Editor,
    ViewId,
};

pub(super) struct Prepared {
    pub(super) transaction: Transaction,
    pub(super) interactive: Option<(ViewId, RenderedSnippet)>,
}

pub(super) fn range(edit: &lsp::DocumentTextEdit) -> lsp::Range {
    match edit {
        lsp::DocumentTextEdit::Plain(edit) => edit.range,
        lsp::DocumentTextEdit::Annotated(edit) => edit.text_edit.range,
        lsp::DocumentTextEdit::Snippet(edit) => edit.range,
    }
}

fn plain(edit: &lsp::DocumentTextEdit) -> Option<&lsp::TextEdit> {
    match edit {
        lsp::DocumentTextEdit::Plain(edit) => Some(edit),
        lsp::DocumentTextEdit::Annotated(edit) => Some(&edit.text_edit),
        lsp::DocumentTextEdit::Snippet(_) => None,
    }
}

/// Annotation approval is checked before batching; IDs do not distinguish edits
/// that otherwise have the same effect at several cursors.
pub(super) fn same_edit(left: &lsp::DocumentTextEdit, right: &lsp::DocumentTextEdit) -> bool {
    match (left, right) {
        (lsp::DocumentTextEdit::Snippet(left), lsp::DocumentTextEdit::Snippet(right)) => {
            left.range == right.range && left.snippet == right.snippet
        }
        _ => plain(left)
            .zip(plain(right))
            .is_some_and(|(left, right)| left == right),
    }
}

pub(super) fn prepare(
    editor: &Editor,
    doc: &Document,
    edits: Vec<lsp::DocumentTextEdit>,
    encoding: OffsetEncoding,
) -> Result<Prepared, ApplyEditErrorKind> {
    let mut expanded: Vec<_> = edits
        .iter()
        .map(|edit| {
            plain(edit).cloned().unwrap_or_else(|| lsp::TextEdit {
                range: range(edit),
                new_text: String::new(),
            })
        })
        .collect();
    if !edits
        .iter()
        .any(|edit| matches!(edit, lsp::DocumentTextEdit::Snippet(_)))
    {
        return Ok(Prepared {
            transaction: generate_transaction_from_edits(doc.text(), expanded, encoding)?,
            interactive: None,
        });
    }
    // Validate original ranges together before rendering: snippets cannot hide
    // overlap, reversed ranges, or positions inside a UTF-16 surrogate pair.
    let ranges = expanded
        .iter()
        .map(|edit| lsp::TextEdit {
            range: edit.range,
            new_text: String::new(),
        })
        .collect();
    generate_transaction_from_edits(doc.text(), ranges, encoding)?;
    let view = editor
        .tree
        .try_get(editor.tree.focus())
        .filter(|view| view.doc == doc.id())
        .map(|view| view.id);
    let chosen = view.and_then(|_| {
        edits
            .iter()
            .position(|edit| matches!(edit, lsp::DocumentTextEdit::Snippet(_)))
    });
    let mut order: Vec<_> = (0..edits.len()).collect();
    order.sort_by_key(|&index| range(&edits[index]).start);
    let mut offset = 0i128;
    let mut interactive = None;
    for index in order {
        let bounds =
            lsp_client::util::lsp_range_to_range(doc.text(), range(&edits[index]), encoding)
                .expect("validated original edit range");
        if let lsp::DocumentTextEdit::Snippet(edit) = &edits[index] {
            if edit.snippet.kind != "snippet" {
                return Err(ApplyEditErrorKind::InvalidSnippetKind(
                    edit.snippet.kind.clone(),
                ));
            }
            let snippet =
                Snippet::parse(&edit.snippet.value).map_err(ApplyEditErrorKind::InvalidSnippet)?;
            let selection = Selection::single(bounds.from(), bounds.to());
            let mut context = editor.snippet_context_with_mode(
                doc,
                doc.text(),
                &selection,
                &snippet,
                Mode::Select,
            );
            let (transaction, _, mut rendered) = snippet.render(
                doc.text(),
                &selection,
                |range| (range.from(), range.to()),
                &mut context,
            );
            expanded[index].new_text = transaction
                .changes_iter()
                .next()
                .and_then(|(_, _, text)| text)
                .unwrap_or_default()
                .to_string();
            if chosen == Some(index) {
                // Rendering used the original snapshot. Shift only by preceding
                // edits, so equal-position inserts retain their protocol order.
                for range in &mut rendered.ranges {
                    range.start = (range.start as i128 + offset) as usize;
                    range.end = (range.end as i128 + offset) as usize;
                }
                for stop in &mut rendered.tabstops {
                    for entry in &mut stop.ranges {
                        entry.range.start = (entry.range.start as i128 + offset) as usize;
                        entry.range.end = (entry.range.end as i128 + offset) as usize;
                    }
                }
                interactive = Some((view.unwrap(), rendered));
            }
        }
        offset += expanded[index].new_text.chars().count() as i128
            - (bounds.to() - bounds.from()) as i128;
    }
    Ok(Prepared {
        transaction: generate_transaction_from_edits(doc.text(), expanded, encoding)?,
        interactive,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(start: u32, end: u32, value: &str) -> lsp::DocumentTextEdit {
        lsp::SnippetTextEdit {
            range: lsp::Range::new(lsp::Position::new(0, start), lsp::Position::new(0, end)),
            snippet: lsp::StringValue {
                kind: "snippet".into(),
                value: value.into(),
            },
            annotation_id: None,
        }
        .into()
    }

    fn action(uri: lsp::Url, edits: Vec<lsp::DocumentTextEdit>) -> lsp::WorkspaceEdit {
        lsp::WorkspaceEdit {
            document_changes: Some(lsp::DocumentChanges::Edits(vec![lsp::TextDocumentEdit {
                text_document: lsp::OptionalVersionedTextDocumentIdentifier { uri, version: None },
                edits,
            }])),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn batched_snippet_actions_deduplicate_shared_edits_and_use_one_undo_step() {
        let (mut editor, dir) = crate::editor::tests::editor("#[|a]#b\n");
        let id = current_ref!(editor).1.id();
        editor.set_doc_path(id, &dir.path().join("source.txt"));
        let uri = current_ref!(editor).1.url().unwrap();
        let import: lsp::DocumentTextEdit = lsp::TextEdit {
            range: lsp::Range::default(),
            new_text: "import\n".into(),
        }
        .into();
        editor
            .apply_code_action_edits(
                OffsetEncoding::Utf8,
                vec![
                    action(
                        uri.clone(),
                        vec![import.clone(), snippet(0, 1, "${1:one}$0")],
                    ),
                    action(uri, vec![import, snippet(1, 2, "${1:two}$0")]),
                ],
            )
            .unwrap();
        let (view, doc) = current!(editor);
        assert_eq!(doc.text(), "import\nonetwo\n");
        assert_eq!(doc.selection(view.id), &Selection::single(7, 10));
        assert!(doc.active_snippet().is_some());
        assert!(crate::editing::undo(doc, view.id));
        assert_eq!(doc.text(), "ab\n");
        assert!(!crate::editing::undo(doc, view.id));
    }

    #[tokio::test]
    async fn batched_actions_preflight_every_snippet_before_editing_any_document() {
        let (mut editor, dir) = crate::editor::tests::editor("#[|a]#b\n");
        let id = current_ref!(editor).1.id();
        editor.set_doc_path(id, &dir.path().join("source.txt"));
        let uri = current_ref!(editor).1.url().unwrap();
        let other = dir.path().join("other.txt");
        std::fs::write(&other, "other\n").unwrap();
        let invalid = editor.apply_code_action_edits(
            OffsetEncoding::Utf8,
            vec![
                action(uri, vec![snippet(0, 1, "${1:changed}$0")]),
                action(
                    <lsp::Url as lsp_client::UriExt>::from_file_path(&other).unwrap(),
                    vec![snippet(0, 0, "${1:${1:recursive}}")],
                ),
            ],
        );
        assert!(invalid.is_err());
        let (view, doc) = current!(editor);
        assert_eq!(doc.text(), "ab\n");
        assert!(doc.active_snippet().is_none());
        assert!(!crate::editing::undo(doc, view.id));
        assert_eq!(std::fs::read_to_string(other).unwrap(), "other\n");
    }
}

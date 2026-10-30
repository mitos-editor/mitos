//! Undoable conflict resolution operates only on the current document snapshot.

use std::sync::Arc;

use anyhow::{ensure, Result};
use editor_core::{Rope, Selection, Transaction};
use vcs::conflict::{self, Resolution};

use super::request::SelectionRequest;
use crate::{action, editor::Action, Document, DocumentId, Editor};

fn snapshot(editor: &Editor) -> Result<String> {
    let (_, doc) = crate::current_ref!(editor);
    ensure!(!doc.is_binary(), "Conflict actions require a text buffer");
    ensure!(
        doc.text().len() <= vcs::MAX_REVIEW_BYTES,
        "Buffer too large to inspect conflicts"
    );
    Ok(doc.text().to_string())
}

pub fn navigate(editor: &mut Editor, forward: bool, count: usize) -> Result<()> {
    let text = snapshot(editor)?;
    let conflicts = conflict::parse(&text)?;
    ensure!(!conflicts.is_empty(), "No conflict markers in this buffer");
    let scrolloff = editor.config().scroll.padding;
    let (view, doc) = crate::current!(editor);
    let cursor = doc.text().char_to_byte_idx(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
    );
    let index = if forward {
        conflicts
            .iter()
            .position(|c| c.whole.start > cursor)
            .unwrap_or(0)
    } else {
        conflicts
            .iter()
            .rposition(|c| c.whole.start < cursor)
            .unwrap_or(conflicts.len() - 1)
    };
    let extra = count.saturating_sub(1) % conflicts.len();
    let index = if forward {
        (index + extra) % conflicts.len()
    } else {
        (index + conflicts.len() - extra) % conflicts.len()
    };
    let pos = doc.text().byte_to_char_idx(conflicts[index].whole.start);
    doc.set_selection(view.id, Selection::point(pos));
    view.ensure_cursor_in_view(doc, scrolloff);
    editor.set_status(format!("Conflict {} of {}", index + 1, conflicts.len()));
    Ok(())
}

pub fn resolve(editor: &mut Editor, choice: Resolution) -> Result<()> {
    let text = snapshot(editor)?;
    let conflicts = conflict::parse(&text)?;
    let (view, doc) = crate::current!(editor);
    let cursor = doc.text().char_to_byte_idx(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
    );
    let conflict = conflicts
        .iter()
        .find(|c| c.whole.contains(&cursor))
        .ok_or_else(|| anyhow::anyhow!("Place the cursor inside a conflict first"))?;
    let start = doc.text().byte_to_char_idx(conflict.whole.start);
    let end = doc.text().byte_to_char_idx(conflict.whole.end);
    let replacement = conflict.replacement(&text, choice);
    let transaction = Transaction::change(
        doc.text(),
        [(start, end, Some(replacement.into()))].into_iter(),
    );
    crate::editing::apply(doc, &transaction, view.id)?;
    doc.commit_changes();
    doc.set_selection(view.id, Selection::single(start, start));
    view.sync_changes(doc);
    editor.set_status(format!(
        "{} conflicts remaining; review, save, and stage the resolved file",
        conflicts.len() - 1
    ));
    Ok(())
}

/// Local actions share the code-action menu and its captured selection. The target
/// is checked again on execution because the menu can outlive its document snapshot.
pub(crate) fn actions(editor: &Editor, target: Arc<SelectionRequest>) -> Vec<action::Action> {
    let Some((view, doc)) = target.get(editor) else {
        return Vec::new();
    };
    if doc.validate_editable().is_err() {
        return Vec::new();
    }
    let Ok(text) = snapshot(editor) else {
        return Vec::new();
    };
    let Ok(conflicts) = conflict::parse(&text) else {
        return Vec::new();
    };
    let cursor = doc.text().char_to_byte_idx(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
    );
    if !conflicts
        .iter()
        .any(|conflict| conflict.whole.contains(&cursor))
    {
        return Vec::new();
    }
    [
        ("Accept current", Resolution::Current),
        ("Accept incoming", Resolution::Incoming),
        ("Accept both (current then incoming)", Resolution::Both),
    ]
    .into_iter()
    .map(|(title, choice)| {
        let target = target.clone();
        action::Action::new(title, 100, move |editor| {
            if !target
                .get(editor)
                .is_some_and(|(view, _)| editor.tree.focus() == view.id)
            {
                editor.set_error(|| "Conflict action is no longer current");
                return;
            }
            if let Err(error) = resolve(editor, choice) {
                editor.set_error(|| error.to_string());
            }
        })
    })
    .collect()
}

pub fn open_source(editor: &mut Editor, name: String, text: Rope) -> DocumentId {
    let doc = Document::from(
        text,
        None,
        editor.config.clone(),
        editor.syn_loader.load_full(),
    )
    .with_readonly_name(name);
    editor.new_file_from_document(Action::VerticalSplit, doc)
}

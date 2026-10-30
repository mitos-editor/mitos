//! Undoable conflict resolution operates only on the current document snapshot.

use std::sync::Arc;

use anyhow::{ensure, Result};
use editor_core::{Rope, Selection, Transaction};
use vcs::conflict::{self, Resolution};

mod presentation;
pub use presentation::{ConflictMarker, ConflictPresentation, ConflictSide};

use super::request::SelectionRequest;
use crate::{action, editor::Action, Document, DocumentId, Editor};

fn regions(editor: &Editor) -> Result<&[conflict::Conflict]> {
    let (_, doc) = crate::current_ref!(editor);
    ensure!(!doc.is_binary(), "Conflict actions require a text buffer");
    ensure!(
        doc.text().len() <= vcs::MAX_REVIEW_BYTES,
        "Buffer too large to inspect conflicts"
    );
    let state = doc.conflict_presentation();
    if let Some(error) = &state.error {
        anyhow::bail!("{error}");
    }
    Ok(&state.regions)
}

pub fn navigate(editor: &mut Editor, forward: bool, count: usize) -> Result<()> {
    let conflicts = regions(editor)?;
    ensure!(!conflicts.is_empty(), "No conflict markers in this buffer");
    let scrolloff = editor.config().scroll.padding;
    let (view, doc) = crate::current_ref!(editor);
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
    let total = conflicts.len();
    let (view, doc) = crate::current!(editor);
    doc.set_selection(view.id, Selection::point(pos));
    view.ensure_cursor_in_view(doc, scrolloff);
    editor.set_status(format!("Conflict {} of {}", index + 1, total));
    Ok(())
}

pub fn resolve(editor: &mut Editor, choice: Resolution) -> Result<()> {
    let conflicts = regions(editor)?;
    let (view, doc) = crate::current_ref!(editor);
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
    let text = doc.text();
    let replacement = match choice {
        Resolution::Current => text.slice(conflict.current.clone()).to_string(),
        Resolution::Incoming => text.slice(conflict.incoming.clone()).to_string(),
        Resolution::Both => format!(
            "{}{}",
            text.slice(conflict.current.clone()),
            text.slice(conflict.incoming.clone())
        ),
    };
    let remaining = conflicts.len() - 1;
    let transaction = Transaction::change(
        doc.text(),
        [(start, end, Some(replacement.into()))].into_iter(),
    );
    let (view, doc) = crate::current!(editor);
    crate::editing::apply(doc, &transaction, view.id)?;
    doc.commit_changes();
    doc.set_selection(view.id, Selection::single(start, start));
    view.sync_changes(doc);
    editor.set_status(format!(
        "{} conflicts remaining; review, save, and stage the resolved file",
        remaining
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
    let Ok(conflicts) = regions(editor) else {
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

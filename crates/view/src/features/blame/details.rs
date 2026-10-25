//! View-bound presentation requests sharing the document's background blame work.

use std::sync::Arc;

use event::{register_hook, TaskController};
use parking_lot::Mutex;
use vcs::{FileBlame, LineBlame};

use super::LineBlameError;
use crate::{events::DocumentFocusLost, features::request::SelectionRequest, Editor};

#[derive(Default)]
pub(super) struct DetailsSession {
    cancel: TaskController,
    target: Option<Target>,
}

struct Target {
    selection: SelectionRequest,
    line: u32,
    trust_full: bool,
}

impl Target {
    fn document<'a>(&self, editor: &'a Editor) -> Option<&'a crate::Document> {
        let (view, doc) = self.selection.get(editor)?;
        let trust_full = editor
            .workspace_trust
            .query(doc.workspace_root(), workspace::trust::TrustQuery::Git)
            .is_trusted();
        (editor.tree.focus() == view.id && trust_full == self.trust_full).then_some(doc)
    }
}

/// Request details for the primary cursor without enabling inline annotations.
pub fn request_details(editor: &mut Editor) {
    let handler = editor.handlers.blame.clone();
    let mut session = handler.details.lock();
    session.target = None;
    session.cancel.cancel();
    let (view, doc) = crate::current!(editor);
    if doc.is_binary() || doc.path().is_none() {
        editor.set_error(|| "Git blame requires a text file with a path");
        return;
    }
    let trust_full = editor
        .workspace_trust
        .query(doc.workspace_root(), workspace::trust::TrustQuery::Git)
        .is_trusted();
    if doc.update_blame_trust(trust_full) {
        doc.refresh_vcs(trust_full);
    }
    session.target = Some(Target {
        line: doc.cursor_line(view.id) as u32,
        selection: SelectionRequest::new(doc, view.id, session.cancel.restart(), Vec::new()),
        trust_full,
    });
    if doc.file_blame().is_none() {
        handler.request(doc, trust_full, None);
        editor.set_status("Requested blame for this file...");
    }
    editor.redraw().request();
}

/// A frontend result, validated again immediately before presentation.
pub struct BlameDetailsUpdate {
    owner: Arc<Mutex<DetailsSession>>,
    target: Target,
    snapshot: Option<Arc<FileBlame>>,
    result: Result<LineBlame, String>,
}

impl std::fmt::Debug for BlameDetailsUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlameDetailsUpdate").finish_non_exhaustive()
    }
}

impl BlameDetailsUpdate {
    pub fn resolve(self, editor: &Editor) -> Option<Result<LineBlame, String>> {
        if !Arc::ptr_eq(&self.owner, &editor.handlers.blame.details) {
            return None;
        }
        let doc = self.target.document(editor)?;
        if let Some(snapshot) = self.snapshot {
            let current = doc.file_blame()?.as_ref().ok()?;
            if !Arc::ptr_eq(&snapshot, current) {
                return None;
            }
        }
        Some(self.result)
    }
}

pub fn next_update(editor: &Editor) -> Option<BlameDetailsUpdate> {
    let owner = editor.handlers.blame.details.clone();
    let mut session = owner.lock();
    let target = session.target.as_ref()?;
    let Some(doc) = target.document(editor) else {
        session.target = None;
        return None;
    };
    let result = match doc.line_blame_details(target.line) {
        Err(LineBlameError::NotReadyYet) => return None,
        result => result.cloned().map_err(|err| err.to_string()),
    };
    let snapshot = doc
        .file_blame()
        .and_then(|result| result.as_ref().ok())
        .cloned();
    let target = session.target.take()?;
    drop(session);
    Some(BlameDetailsUpdate {
        owner,
        target,
        snapshot,
        result,
    })
}

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        let mut session = event.editor.handlers.blame.details.lock();
        session.cancel.cancel();
        session.target = None;
        Ok(())
    });
}

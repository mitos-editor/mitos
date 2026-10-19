//! A document-bound source capability. Only the editor can mint adapter source handles.
use super::local_path;
use crate::{callbacks::EditorCallbackSender, DocumentId, Editor};
use dap::{DebugAdapterId, StackFrame};
use event::TaskHandle;
use std::path::{Path, PathBuf};

#[derive(Clone)]
enum Origin {
    File(PathBuf),
    Adapter {
        session: DebugAdapterId,
        reference: usize,
        id: ::debugger::SourceId,
        current: TaskHandle,
    },
}

/// A debugger source bound to its document, editor, and adapter source version.
/// A source-change event, restart, document close, or session removal revokes adapter handles.
#[derive(Clone)]
pub struct DebuggerSource {
    document: DocumentId,
    owner: EditorCallbackSender,
    origin: Origin,
}

impl DebuggerSource {
    pub fn is_current(&self, editor: &Editor) -> bool {
        if !self.owner.same_destination(editor.callback_sender()) {
            return false;
        }
        let Some(doc) = editor.document(self.document) else {
            return false;
        };
        match &self.origin {
            Origin::File(path) => doc.path() == Some(path),
            Origin::Adapter {
                session,
                reference,
                id,
                current,
            } => {
                !current.is_canceled()
                    && !editor.debugger.is_terminating(*session)
                    && editor
                        .debugger
                        .get_session(*session)
                        .and_then(|session| session.sources.entries.get(reference))
                        .is_some_and(|cached| {
                            cached.id == *id && cached.document == Some(self.document)
                        })
            }
        }
    }

    pub fn local_path(&self) -> Option<&Path> {
        match &self.origin {
            Origin::File(path) => Some(path),
            Origin::Adapter { .. } => None,
        }
    }

    pub(in crate::debugger) fn adapter(&self) -> Option<(DebugAdapterId, ::debugger::SourceId)> {
        match self.origin {
            Origin::Adapter { session, id, .. } => Some((session, id)),
            Origin::File(_) => None,
        }
    }
    pub(in crate::debugger) fn breakpoint_source(&self) -> ::debugger::BreakpointSource {
        match &self.origin {
            Origin::File(path) => ::debugger::BreakpointSource::File(path.clone()),
            Origin::Adapter { id, .. } => ::debugger::BreakpointSource::Adapter(*id),
        }
    }

    pub(super) fn matches_frame(&self, session: DebugAdapterId, frame: &StackFrame) -> bool {
        match &self.origin {
            Origin::File(path) => local_path(frame) == Some(path),
            Origin::Adapter {
                session: owner,
                reference,
                ..
            } => {
                *owner == session
                    && frame
                        .source
                        .as_ref()
                        .and_then(|source| source.source_reference)
                        == Some(*reference)
            }
        }
    }
}

impl Editor {
    /// Resolve a live source for debugger operations. Historical adapter snapshots have none.
    pub fn debugger_source(&self, document: DocumentId) -> Option<DebuggerSource> {
        let doc = self.document(document)?;
        let origin = if let Some(path) = doc.path() {
            Origin::File(path.to_owned())
        } else {
            self.debugger.sessions.iter().find_map(|(id, session)| {
                if self.debugger.is_terminating(id) {
                    return None;
                }
                session
                    .sources
                    .entries
                    .iter()
                    .find_map(|(reference, cached)| {
                        (cached.document == Some(document)).then(|| Origin::Adapter {
                            session: id,
                            reference: *reference,
                            id: cached.id,
                            current: cached.current.clone(),
                        })
                    })
            })?
        };
        Some(DebuggerSource {
            document,
            owner: self.callback_sender().clone(),
            origin,
        })
    }
}

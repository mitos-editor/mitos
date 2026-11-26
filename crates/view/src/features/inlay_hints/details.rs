//! User-initiated access to full hints. Display annotations are never action targets.

use std::{
    future::Future,
    sync::{Arc, Weak},
};

use editor_core::Selection;
use event::cancelable_future;
use language_config::LanguageServerFeature;
use lsp_client::{lsp, LanguageServerId, OffsetEncoding};
use stdx::rope::RopeExt as _;

use crate::{
    callbacks::EditorCallbackSender, features::request::SelectionRequest, DocumentId, Editor,
    ViewId,
};

/// Opaque identity for one hint in a fresh document/view cache.
#[derive(Clone)]
pub struct InlayHintTarget {
    owner: Weak<EditorCallbackSender>,
    document: DocumentId,
    view: ViewId,
    generation: Arc<()>,
    index: usize,
    selection: Selection,
}

impl InlayHintTarget {
    /// Check the editor, document snapshot, provider, view and original selection.
    pub fn is_current(&self, editor: &Editor) -> bool {
        if !self
            .owner
            .ptr_eq(&Arc::downgrade(&editor.handlers.inlay_hints.callbacks))
            || !editor.config().lsp.enable
            || !editor.config().lsp.inlay_hints.enable
            || editor.tree.focus() != self.view
            || !editor
                .tree
                .try_get(self.view)
                .is_some_and(|view| view.doc == self.document)
        {
            return false;
        }
        let Some(doc) = editor.document(self.document) else {
            return false;
        };
        let Some(state) = doc
            .view_states
            .get(&self.view)
            .map(|state| &state.inlay_hints)
        else {
            return false;
        };
        let Some(source) = &state.source else {
            return false;
        };
        Arc::ptr_eq(&self.generation, &state.generation)
            && source.snapshot.is_current(&doc.buffer)
            && source.key.uri == doc.uri()
            && doc.try_selection(self.view) == Some(&self.selection)
            && doc
                .language_servers_with_feature(LanguageServerFeature::InlayHints)
                .next()
                .is_some_and(|server| {
                    server.id() == source.key.server
                        && editor.language_server_by_id(server.id()).is_some()
                })
            && self.index < state.hints.len()
    }
}

/// A candidate with its full, untruncated label, in server response order.
#[derive(Clone)]
pub struct InlayHintItem {
    pub target: InlayHintTarget,
    pub label: String,
}

/// Hints on the cursor's current document line. Stale mapped hints are excluded.
pub fn hints_on_line(editor: &Editor, view: ViewId, line: usize) -> Vec<InlayHintItem> {
    let Some(view) = editor.tree.try_get(view) else {
        return Vec::new();
    };
    let Some(doc) = editor.document(view.doc) else {
        return Vec::new();
    };
    let Some(state) = doc
        .view_states
        .get(&view.id)
        .map(|state| &state.inlay_hints)
    else {
        return Vec::new();
    };
    let Some(source) = &state.source else {
        return Vec::new();
    };
    state
        .hints
        .iter()
        .enumerate()
        .filter_map(|(index, hint)| {
            let target = InlayHintTarget {
                owner: source.owner.clone(),
                document: doc.id(),
                view: view.id,
                generation: state.generation.clone(),
                index,
                selection: doc.selection(view.id).clone(),
            };
            (target.is_current(editor) && doc.text().char_to_line_idx(hint.char_idx) == line).then(
                || InlayHintItem {
                    target,
                    label: super::label(&hint.hint.label),
                },
            )
        })
        .collect()
}

/// An interaction result, guarded until the frontend displays or uses it.
pub struct ResolvedInlayHint {
    target: InlayHintTarget,
    request: SelectionRequest,
    hint: lsp::InlayHint,
    language_server_id: LanguageServerId,
    offset_encoding: OffsetEncoding,
    resolved: bool,
}

impl ResolvedInlayHint {
    pub fn hint(&self) -> &lsp::InlayHint {
        &self.hint
    }

    pub fn language_server_id(&self) -> LanguageServerId {
        self.language_server_id
    }

    pub fn offset_encoding(&self) -> OffsetEncoding {
        self.offset_encoding
    }

    pub fn document_id(&self) -> DocumentId {
        self.target.document
    }

    pub fn view_id(&self) -> ViewId {
        self.target.view
    }

    pub fn is_current(&self, editor: &Editor) -> bool {
        self.target.is_current(editor) && self.request.get(editor).is_some()
    }

    /// Retain successful resolution so subsequent interactions do not repeat it.
    pub fn cache(&self, editor: &mut Editor) {
        if self.resolved && self.is_current(editor) {
            let doc = editor.document_mut(self.target.document).unwrap();
            let state = &mut doc.view_state_mut(self.target.view).inlay_hints;
            let hint = &mut state.hints[self.target.index];
            hint.hint = self.hint.clone();
            hint.resolved = true;
        }
    }
}

/// Resolve a selected hint lazily, retaining its originating server and exact data.
/// Failed requests leave eager details usable and can be retried on the next interaction.
pub fn resolve(
    editor: &mut Editor,
    target: InlayHintTarget,
) -> Option<impl Future<Output = Option<ResolvedInlayHint>> + Send + use<>> {
    if !target.is_current(editor) {
        return None;
    }
    let doc = editor.document_mut(target.document)?;
    let state = &mut doc.view_state_mut(target.view).inlay_hints;
    let source = state.source.as_ref()?;
    let server_id = source.key.server;
    let encoding = source.encoding;
    let cached = &state.hints[target.index];
    let hint = cached.hint.clone();
    let was_resolved = cached.resolved;
    let cancel = state.details.restart();
    let request = SelectionRequest::new(doc, target.view, cancel, vec![server_id]);
    let future = (!was_resolved)
        .then(|| {
            editor
                .language_server_by_id(server_id)?
                .resolve_inlay_hint(hint.clone())
        })
        .flatten();
    Some(async move {
        let mut result = ResolvedInlayHint {
            target,
            request,
            hint,
            language_server_id: server_id,
            offset_encoding: encoding,
            resolved: was_resolved || future.is_none(),
        };
        if let Some(future) = future {
            match cancelable_future(future, result.request.cancel_handle()).await? {
                Ok(hint) if same_label(&result.hint, &hint) => {
                    // Only advertised lazy fields may change. Omitted optional
                    // fields retain eager information; data and display fields
                    // always keep their original values.
                    result.hint.tooltip = hint.tooltip.or(result.hint.tooltip);
                    result.hint.text_edits = hint.text_edits.or(result.hint.text_edits);
                    if let (
                        lsp::InlayHintLabel::LabelParts(original),
                        lsp::InlayHintLabel::LabelParts(resolved),
                    ) = (&mut result.hint.label, hint.label)
                    {
                        for (original, resolved) in original.iter_mut().zip(resolved) {
                            original.tooltip = resolved.tooltip.or(original.tooltip.take());
                            original.location = resolved.location.or(original.location.take());
                            original.command = resolved.command.or(original.command.take());
                        }
                    }
                    result.resolved = true;
                }
                Ok(_) => log::warn!("inlay hint resolution changed its position or label"),
                Err(error) => log::error!("inlay hint resolution failed: {error}"),
            }
        }
        Some(result)
    })
}

fn same_label(original: &lsp::InlayHint, resolved: &lsp::InlayHint) -> bool {
    original.position == resolved.position
        && match (&original.label, &resolved.label) {
            (lsp::InlayHintLabel::String(a), lsp::InlayHintLabel::String(b)) => a == b,
            (lsp::InlayHintLabel::LabelParts(a), lsp::InlayHintLabel::LabelParts(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.value == b.value)
            }
            _ => false,
        }
}

//! Request selection, stale-response checks, and LSP annotation conversion.

use std::sync::{Arc, Weak};

use editor_core::{syntax::config::LanguageServerFeature, text_annotations::InlineAnnotation};
use event::cancelable_future;
use lsp_client::{lsp, OffsetEncoding};

use super::state::{CacheKey, DocumentInlayHints, DocumentInlayHintsId};
use crate::{
    callbacks::EditorCallbackSender, features::request::ViewRequest, Document, Editor, View,
};

struct Request {
    owner: Weak<EditorCallbackSender>,
    target: ViewRequest,
    key: CacheKey,
}

fn viewport(view: &View, doc: &Document) -> DocumentInlayHintsId {
    let text = doc.text();
    let height = view.inner_height(doc);
    let first_visible = text.char_to_line(doc.view_offset(view.id).anchor.min(text.len_chars()));
    DocumentInlayHintsId {
        first_line: first_visible.saturating_sub(height),
        last_line: first_visible
            .saturating_add(height.saturating_mul(2))
            .min(text.len_lines()),
    }
}

impl Request {
    fn is_current(&self, editor: &Editor) -> bool {
        self.owner
            .ptr_eq(&Arc::downgrade(&editor.handlers.inlay_hints.callbacks))
            && editor.config().lsp.enable
            && editor.config().lsp.inlay_hints.enable
            && self.target.get(editor).is_some_and(|(view, doc)| {
                viewport(view, doc) == self.key.viewport
                    && doc
                        .language_servers_with_feature(LanguageServerFeature::InlayHints)
                        .next()
                        .is_some_and(|server| server.id() == self.key.server)
            })
    }
}

/// Refresh each visible view. Pending and completed requests share the same cache key,
/// preventing repeated idle ticks from restarting an unchanged request.
pub fn refresh(editor: &mut Editor) {
    if !editor.config().lsp.enable || !editor.config().lsp.inlay_hints.enable {
        return;
    }
    let callbacks = editor.handlers.inlay_hints.callbacks.clone();
    let views: Vec<_> = editor
        .tree
        .views()
        .map(|(view, _)| (view.id, view.doc))
        .collect();
    for (view_id, doc_id) in views {
        let view = editor.tree.get(view_id);
        let doc = &editor.documents[&doc_id];
        let Some(server) = doc
            .language_servers_with_feature(LanguageServerFeature::InlayHints)
            .next()
        else {
            let doc = editor.documents.get_mut(&doc_id).unwrap();
            doc.inlay_hints.remove_view(view_id);
            continue;
        };
        let key = CacheKey {
            version: doc.version(),
            uri: doc.uri(),
            server: server.id(),
            viewport: viewport(view, doc),
        };
        if doc.inlay_hints.is_current(view_id, &key) {
            continue;
        }
        let text = doc.text();
        let range = lsp_client::util::range_to_lsp_range(
            text,
            editor_core::Range::new(
                text.line_to_char(key.viewport.first_line),
                text.line_to_char(key.viewport.last_line),
            ),
            server.offset_encoding(),
        );
        let encoding = server.offset_encoding();
        let Some(future) = server.text_document_range_inlay_hints(doc.identifier(), range, None)
        else {
            continue;
        };
        let doc = editor.documents.get_mut(&doc_id).unwrap();
        let cancel = doc.inlay_hints.start(view_id, key.clone());
        let request = Request {
            owner: Arc::downgrade(&callbacks),
            target: ViewRequest::new(doc, view_id, cancel, vec![key.server]),
            key,
        };
        let callbacks = callbacks.clone();
        tokio::spawn(async move {
            let Some(result) = cancelable_future(future, request.target.cancel_handle()).await
            else {
                return;
            };
            callbacks
                .send(move |editor| {
                    if !request.is_current(editor) {
                        return;
                    }
                    let view_id = request.target.view_id();
                    let doc = editor.document_mut(request.target.document_id()).unwrap();
                    match result {
                        Ok(hints) => {
                            let hints = annotations(
                                doc.text(),
                                request.key.viewport,
                                encoding,
                                doc.config.load().lsp.inlay_hints.length_limit,
                                hints.unwrap_or_default(),
                            );
                            doc.inlay_hints.complete(view_id, hints);
                            event::request_redraw();
                        }
                        Err(error) => {
                            log::error!("inlay hints request failed: {error}");
                            doc.inlay_hints.retry(view_id);
                        }
                    }
                })
                .await;
        });
    }
}

fn annotations(
    text: &editor_core::Rope,
    id: DocumentInlayHintsId,
    offset_encoding: OffsetEncoding,
    length_limit: Option<std::num::NonZeroU8>,
    mut hints: Vec<lsp::InlayHint>,
) -> DocumentInlayHints {
    // Most language servers will already send them sorted but ensure this is the case to
    // avoid errors on our end.
    hints.sort_by_key(|inlay_hint| inlay_hint.position);

    let mut padding_before_inlay_hints = Vec::new();
    let mut type_inlay_hints = Vec::new();
    let mut parameter_inlay_hints = Vec::new();
    let mut other_inlay_hints = Vec::new();
    let mut padding_after_inlay_hints = Vec::new();

    for hint in hints {
        let char_idx = match lsp_client::util::lsp_pos_to_pos(text, hint.position, offset_encoding)
        {
            Some(pos) => pos,
            // Skip inlay hints that have no "real" position
            None => continue,
        };

        let mut label = match hint.label {
            lsp::InlayHintLabel::String(s) => s,
            lsp::InlayHintLabel::LabelParts(parts) => parts.into_iter().map(|p| p.value).collect(),
        };
        // Truncate the hint if too long
        if let Some(limit) = length_limit {
            // Limit on displayed width
            use editor_core::unicode::{segmentation::UnicodeSegmentation, width::UnicodeWidthStr};

            let width = label.width();
            let limit = limit.get().into();
            if width > limit {
                let mut floor_boundary = 0;
                let mut acc = 0;
                for (i, grapheme_cluster) in label.grapheme_indices(true) {
                    acc += grapheme_cluster.width();

                    if acc > limit {
                        floor_boundary = i;
                        break;
                    }
                }

                label.truncate(floor_boundary);
                label.push('…');
            }
        }

        let inlay_hints_vec = match hint.kind {
            Some(lsp::InlayHintKind::TYPE) => &mut type_inlay_hints,
            Some(lsp::InlayHintKind::PARAMETER) => &mut parameter_inlay_hints,
            // We can't warn on unknown kind here since LSPs are free to set it or not, for
            // example Rust Analyzer does not: every kind will be `None`.
            _ => &mut other_inlay_hints,
        };

        if let Some(true) = hint.padding_left {
            padding_before_inlay_hints.push(InlineAnnotation::new(char_idx, " "));
        }

        inlay_hints_vec.push(InlineAnnotation::new(char_idx, label));

        if let Some(true) = hint.padding_right {
            padding_after_inlay_hints.push(InlineAnnotation::new(char_idx, " "));
        }
    }
    DocumentInlayHints {
        id,
        type_inlay_hints,
        parameter_inlay_hints,
        other_inlay_hints,
        padding_before_inlay_hints,
        padding_after_inlay_hints,
    }
}

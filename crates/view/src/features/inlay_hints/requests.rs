//! Request selection, stale-response checks, and LSP annotation conversion.

use std::sync::{Arc, Weak};
use stdx::rope::RopeExt as _;

use event::cancelable_future;
use language_config::LanguageServerFeature;
use lsp_client::{lsp, OffsetEncoding};
use text_layout::text_annotations::InlineAnnotation;

use super::state::{
    CacheKey, CachedHint, DisplayedHint, DocumentInlayHints, DocumentInlayHintsId, HintStyle,
};
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
    let first_visible =
        text.char_to_line_idx(doc.view_offset(view.id).anchor.min(text.len_chars()));
    DocumentInlayHintsId {
        first_line: first_visible.saturating_sub(height),
        last_line: first_visible
            .saturating_add(height.saturating_mul(2))
            .min(text.len_lines(stdx::rope::LINE_TYPE)),
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
            if let Some(state) = doc.view_states.get_mut(&view_id) {
                state.inlay_hints = Default::default();
            }
            continue;
        };
        let key = CacheKey {
            version: doc.version(),
            uri: doc.uri(),
            server: server.id(),
            viewport: viewport(view, doc),
        };
        if doc
            .view_states
            .get(&view_id)
            .is_some_and(|state| state.inlay_hints.is_current(&key))
        {
            continue;
        }
        let text = doc.text();
        let range = lsp_client::util::range_to_lsp_range(
            text,
            editor_core::Range::new(
                text.line_to_char_idx(key.viewport.first_line),
                text.line_to_char_idx(key.viewport.last_line),
            ),
            server.offset_encoding(),
        );
        let encoding = server.offset_encoding();
        let Some(future) = server.text_document_range_inlay_hints(doc.identifier(), range, None)
        else {
            continue;
        };
        let doc = editor.documents.get_mut(&doc_id).unwrap();
        let cancel = doc.view_state_mut(view_id).inlay_hints.start(key.clone());
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
                            let snapshot = doc.snapshot();
                            let (hints, retained) = annotations(
                                doc.text(),
                                request.key.viewport,
                                encoding,
                                doc.config.load().lsp.inlay_hints.length_limit,
                                hints.unwrap_or_default(),
                            );
                            doc.view_states
                                .get_mut(&view_id)
                                .expect("validated inlay request owns its view state")
                                .inlay_hints
                                .complete(
                                    hints,
                                    retained,
                                    snapshot,
                                    encoding,
                                    request.owner.clone(),
                                );
                            editor.redraw().request();
                        }
                        Err(error) => {
                            log::error!("inlay hints request failed: {error}");
                            doc.view_states
                                .get_mut(&view_id)
                                .expect("validated inlay request owns its view state")
                                .inlay_hints
                                .retry();
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
) -> (DocumentInlayHints, Vec<CachedHint>) {
    // Most language servers will already send them sorted but ensure this is the case to
    // avoid errors on our end.
    hints.sort_by_key(|inlay_hint| inlay_hint.position);

    let mut padding_before_inlay_hints = Vec::new();
    let mut type_inlay_hints = Vec::new();
    let mut parameter_inlay_hints = Vec::new();
    let mut other_inlay_hints = Vec::new();
    let mut padding_after_inlay_hints = Vec::new();
    let mut retained = Vec::new();
    let mut ordered = Vec::new();
    let mut styles = Vec::new();
    let mut labels = Vec::new();

    for hint in hints {
        let char_idx = match lsp_client::util::lsp_pos_to_pos(text, hint.position, offset_encoding)
        {
            Some(pos) => pos,
            // Skip inlay hints that have no "real" position
            None => continue,
        };

        let mut label = super::label(&hint.label);
        let mut label_bytes = label.len();
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
                label_bytes = floor_boundary;
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
            ordered.push(InlineAnnotation::new(char_idx, " "));
            styles.push(HintStyle::Padding);
            labels.push(None);
        }

        ordered.push(InlineAnnotation::new(char_idx, label.clone()));
        labels.push(Some(DisplayedHint {
            hint: retained.len(),
            label_bytes,
        }));
        styles.push(match hint.kind {
            Some(lsp::InlayHintKind::TYPE) => HintStyle::Type,
            Some(lsp::InlayHintKind::PARAMETER) => HintStyle::Parameter,
            _ => HintStyle::Other,
        });
        inlay_hints_vec.push(InlineAnnotation::new(char_idx, label));

        if let Some(true) = hint.padding_right {
            padding_after_inlay_hints.push(InlineAnnotation::new(char_idx, " "));
            ordered.push(InlineAnnotation::new(char_idx, " "));
            styles.push(HintStyle::Padding);
            labels.push(None);
        }
        retained.push(CachedHint {
            char_idx,
            hint,
            resolved: false,
        });
    }
    (
        DocumentInlayHints {
            id,
            type_inlay_hints,
            parameter_inlay_hints,
            other_inlay_hints,
            padding_before_inlay_hints,
            padding_after_inlay_hints,
            ordered,
            styles,
            labels,
        },
        retained,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use text_layout::{
        doc_formatter::{DocumentFormatter, TextFormat},
        text_annotations::{Conceal, TextAnnotations},
    };

    #[test]
    fn displayed_label_metadata_excludes_padding_other_sources_and_truncation_ellipsis() {
        let text = editor_core::Rope::from_str("😀 tail");
        let hint = |label| lsp::InlayHint {
            position: lsp::Position::new(0, 3),
            label: lsp::InlayHintLabel::String(label),
            kind: None,
            text_edits: None,
            tooltip: None,
            padding_left: Some(true),
            padding_right: Some(true),
            data: None,
        };
        let (display, retained) = annotations(
            &text,
            DocumentInlayHintsId {
                first_line: 0,
                last_line: 1,
            },
            OffsetEncoding::Utf16,
            Some(3.try_into().unwrap()),
            vec![
                hint("界e\u{301} suffix".into()),
                hint("界e\u{301} suffix".into()),
            ],
        );
        assert_eq!(retained.len(), 2);
        let other = [InlineAnnotation::new(2, "界e\u{301}…")];
        let conceals = [Conceal { range: 1..3 }];
        let mut annotations = TextAnnotations::default();
        annotations
            .add_inline_annotations(&display.ordered, None)
            .add_inline_annotations(&other, None);
        let format = TextFormat::default();
        let glyphs =
            DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &format, &annotations, 0)
                .collect::<Vec<_>>();
        let hits = glyphs
            .into_iter()
            .filter_map(|glyph| display.hint_for_annotation(glyph.inline_annotation?))
            .collect::<Vec<_>>();
        assert_eq!(hits, [(0, 0), (0, 3), (1, 0), (1, 3)]);
        annotations.set_conceals(&conceals);
        assert!(DocumentFormatter::new_at_prev_checkpoint(
            text.slice(..),
            &format,
            &annotations,
            0
        )
        .all(|glyph| glyph
            .inline_annotation
            .and_then(|source| display.hint_for_annotation(source))
            .is_none()));
    }
}

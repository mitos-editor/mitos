//! Cancelable excerpt parsing on the same bounded workers as ordinary documents.

use std::{ops::Range, sync::Arc};

use editor_core::{
    highlight::{HighlightEvent, OverlayHighlights},
    Rope,
};
use event::{cancelable_future, TaskController};
use language::{syntax, Syntax};

use crate::{callbacks::EditorCallbackSender, Document};

#[derive(Default)]
pub(crate) struct ExcerptSyntax {
    pub(super) trees: Option<Vec<Option<Syntax>>>,
    pub(crate) callbacks: Option<EditorCallbackSender>,
    pub(super) controller: TaskController,
}

impl ExcerptSyntax {
    pub(super) fn invalidate(&mut self) {
        self.controller.cancel();
        self.trees = None;
    }
}

impl Document {
    pub(crate) fn request_multibuffer_syntax(&mut self) {
        let loader = self.syn_loader.load_full();
        let snapshot = self.snapshot();
        let id = self.id();
        let Some(multi) = self.multibuffer.as_mut() else {
            return;
        };
        let Some(callbacks) = multi.syntax.callbacks.clone() else {
            return;
        };
        multi.syntax.invalidate();
        let cancel = multi.syntax.controller.restart();
        let excerpts: Vec<_> = multi
            .segments()
            .iter()
            .map(|s| {
                let text: Rope = snapshot.text().slice(s.projection_range.clone()).into();
                let language = multi
                    .source(s.source)
                    .language_name
                    .as_deref()
                    .and_then(|name| loader.language_for_name(name));
                (text, language)
            })
            .collect();
        tokio::spawn(async move {
            let worker_loader = loader.clone();
            let worker_cancel = cancel.clone();
            let result = cancelable_future(
                async move {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    let permit = crate::features::syntax::WORKERS.acquire().await.ok()?;
                    tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        excerpts
                            .into_iter()
                            .map(|(text, language)| {
                                if worker_cancel.is_canceled() {
                                    return None;
                                }
                                language.and_then(|lang| {
                                    Syntax::new(text.slice(..), lang, &worker_loader).ok()
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .await
                    .ok()
                },
                &cancel,
            )
            .await
            .flatten();
            let Some(trees) = result else {
                return;
            };
            callbacks
                .send(move |editor| {
                    let Some(doc) = editor.document_mut(id) else {
                        return;
                    };
                    if cancel.is_canceled() || !snapshot.is_current(&doc.buffer) {
                        return;
                    }
                    if !Arc::ptr_eq(&loader, &doc.syn_loader.load_full()) {
                        doc.request_multibuffer_syntax();
                        return;
                    }
                    loader.refresh_highlights();
                    if let Some(multi) = doc.multibuffer.as_mut() {
                        multi.syntax.trees = Some(trees);
                    }
                })
                .await;
        });
    }

    /// Highlight only visible excerpt slices; parsing never runs on the render path.
    pub fn multibuffer_highlights(
        &self,
        visible: Range<usize>,
        loader: &syntax::Loader,
    ) -> Vec<OverlayHighlights> {
        let Some(multi) = self.multibuffer() else {
            return Vec::new();
        };
        let Some(trees) = multi.syntax.trees.as_ref() else {
            return Vec::new();
        };
        let mut layers: Vec<Vec<_>> = Vec::new();
        for (segment, syntax) in multi.segments().iter().zip(trees) {
            if segment.projection_range.end <= visible.start
                || segment.projection_range.start >= visible.end
            {
                continue;
            }
            let Some(syntax) = syntax else {
                continue;
            };
            let text = self.text().slice(segment.projection_range.clone());
            let start = visible
                .start
                .saturating_sub(segment.projection_range.start)
                .min(text.len_chars());
            let end = visible
                .end
                .saturating_sub(segment.projection_range.start)
                .min(text.len_chars());
            let mut highlighter = syntax.highlighter(
                text,
                loader,
                text.char_to_byte(start) as u32..text.char_to_byte(end) as u32,
            );
            let mut active = Vec::new();
            loop {
                let start_byte = highlighter.next_event_offset();
                if start_byte == u32::MAX {
                    break;
                }
                let (event, highlights) = highlighter.advance();
                if event == HighlightEvent::Refresh {
                    active.clear();
                }
                active.extend(highlights);
                let end_byte = highlighter.next_event_offset().min(text.len_bytes() as u32);
                if start_byte >= end_byte {
                    continue;
                }
                let start = segment.projection_range.start + text.byte_to_char(start_byte as usize);
                let end = segment.projection_range.start + text.byte_to_char(end_byte as usize);
                for (layer, highlight) in active.iter().copied().enumerate() {
                    if layers.len() <= layer {
                        layers.resize_with(layer + 1, Vec::new);
                    }
                    layers[layer].push((highlight, start..end));
                }
            }
        }
        layers
            .into_iter()
            .filter(|layer| !layer.is_empty())
            .map(|highlights| OverlayHighlights::Heterogenous { highlights })
            .collect()
    }
}

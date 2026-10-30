//! Background syntax scheduling and publication on the owning editor.

use std::sync::Arc;

use crate::{callbacks::EditorCallbackSender, document::FirstSyntax, Document};

/// Scheduling handle shared by documents owned by one editor.
#[derive(Clone)]
pub struct SyntaxHandler {
    callbacks: EditorCallbackSender,
}

impl SyntaxHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self { callbacks }
    }

    pub(crate) fn request(&self, doc: &mut Document) {
        let Some(request) = doc.syntax_request() else {
            return;
        };
        let id = doc.id();
        let callbacks = self.callbacks.clone();
        tokio::spawn(async move {
            let request = Arc::new(request);
            let Some(first) = request.compute_first().await else {
                return;
            };
            // Show the root language as soon as it is ready, then add injections.
            let result = match first {
                Ok(FirstSyntax {
                    syntax: Some(syntax),
                    missing,
                }) if !missing.is_empty() => {
                    let shown = request.clone();
                    callbacks
                        .send(move |editor| {
                            if let Some(doc) = editor.document_mut(id) {
                                shown.publish_first(doc, syntax);
                            }
                        })
                        .await;
                    // Publication restarts a stale snapshot, which cancels this request.
                    let Some(result) = request.compute_injections(missing).await else {
                        return;
                    };
                    result
                }
                first => first.map(|first| first.syntax),
            };
            callbacks
                .send(move |editor| {
                    let Some(doc) = editor.document_mut(id) else {
                        return;
                    };
                    if request.complete(doc, result) {
                        editor.refresh_spelling(id);
                    }
                })
                .await;
        });
    }
}

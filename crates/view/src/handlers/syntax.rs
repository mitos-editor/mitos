//! Background syntax scheduling and publication on the owning editor.

use crate::{callbacks::EditorCallbackSender, Document};

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
            let Some(result) = request.compute().await else {
                return;
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

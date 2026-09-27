//! Batch document refreshes after edits without coupling feature request logic.

use std::{collections::HashSet, time::Duration};

use event::AsyncHook;
use tokio::{sync::mpsc::Sender, time::Instant};

use crate::{callbacks::EditorCallbackSender, DocumentId, Editor};

const DOCUMENT_CHANGE_DEBOUNCE: Duration = Duration::from_millis(250);

/// Coalesce edited documents until edits have been quiet for 250ms. Each feature
/// owns a separate queue and validates its requests when the editor runs them.
pub(super) fn debounce_documents(
    callbacks: EditorCallbackSender,
    request: fn(&mut Editor, DocumentId),
) -> Sender<DocumentId> {
    DocumentDebounce {
        callbacks,
        docs: HashSet::new(),
        request,
    }
    .spawn()
}

struct DocumentDebounce {
    callbacks: EditorCallbackSender,
    docs: HashSet<DocumentId>,
    request: fn(&mut Editor, DocumentId),
}

impl AsyncHook for DocumentDebounce {
    type Event = DocumentId;

    fn handle_event(&mut self, doc: DocumentId, _timeout: Option<Instant>) -> Option<Instant> {
        self.docs.insert(doc);
        Some(Instant::now() + DOCUMENT_CHANGE_DEBOUNCE)
    }

    fn finish_debounce(&mut self) {
        let docs = std::mem::take(&mut self.docs);
        let request = self.request;
        self.callbacks.send_blocking(move |editor| {
            for doc in docs {
                request(editor, doc);
            }
        });
    }
}

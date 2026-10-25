//! Batch document refreshes after edits without coupling feature request logic.

use std::{collections::HashSet, hash::Hash, time::Duration};

use event::AsyncHook;
use tokio::{sync::mpsc::Sender, time::Instant};

use crate::{callbacks::EditorCallbackSender, DocumentId, Editor};

const DOCUMENT_CHANGE_DEBOUNCE: Duration = Duration::from_millis(250);

/// Coalesce edited documents until edits have been quiet for 250ms. Each feature
/// owns a separate queue and validates its requests when the editor runs them.
pub(crate) fn debounce_documents(
    callbacks: EditorCallbackSender,
    request: fn(&mut Editor, DocumentId),
) -> Sender<DocumentId> {
    debounce_targets(callbacks, DOCUMENT_CHANGE_DEBOUNCE, request)
}

pub(crate) fn debounce_targets<T: Eq + Hash + Send + Sync + 'static>(
    callbacks: EditorCallbackSender,
    delay: Duration,
    request: fn(&mut Editor, T),
) -> Sender<T> {
    Debounce {
        callbacks,
        delay,
        targets: HashSet::new(),
        request,
    }
    .spawn()
}

struct Debounce<T> {
    callbacks: EditorCallbackSender,
    delay: Duration,
    targets: HashSet<T>,
    request: fn(&mut Editor, T),
}

impl<T: Eq + Hash + Send + Sync + 'static> AsyncHook for Debounce<T> {
    type Event = T;

    fn handle_event(&mut self, target: T, _timeout: Option<Instant>) -> Option<Instant> {
        self.targets.insert(target);
        Some(Instant::now() + self.delay)
    }

    fn finish_debounce(&mut self) {
        let targets = std::mem::take(&mut self.targets);
        let request = self.request;
        self.callbacks.send_blocking(move |editor| {
            for target in targets {
                request(editor, target);
            }
        });
    }
}

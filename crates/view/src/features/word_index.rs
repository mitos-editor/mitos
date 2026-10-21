//! Eventually consistent lexical completion from the latest snapshot of each open document.

use crate::{
    events::{
        ConfigDidChange, DocumentDidChange, DocumentDidClose, DocumentDidOpen,
        DocumentLanguageDidChange,
    },
    DocumentId,
};
use editor_core::{ChangeSet, Rope};
use event::{register_hook, TaskController, TaskHandle};
use foldhash::HashMap;
use parking_lot::{Mutex, RwLock};
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle, time::Instant};

#[derive(Debug)]
enum Event {
    Open(DocumentId, Rope),
    Edit {
        document: DocumentId,
        text: Rope,
        significant: bool,
    },
    Close(DocumentId),
    Reset,
}

#[derive(Debug)]
pub struct Handler {
    index: WordIndex,
    events: mpsc::UnboundedSender<Event>,
    // Blocking scans check this token so dropping an editor cannot hold up shutdown.
    _cancel: TaskController,
}
impl Handler {
    pub(crate) fn index(&self) -> &WordIndex {
        &self.index
    }
    pub fn spawn() -> Self {
        let index = WordIndex::default();
        let (events, rx) = mpsc::unbounded_channel();
        let mut cancel = TaskController::new();
        tokio::spawn(coordinate(index.clone(), rx, cancel.restart()));
        Self {
            index,
            events,
            _cancel: cancel,
        }
    }
    pub(crate) fn document_trigger(&self) -> WordIndexTrigger {
        WordIndexTrigger {
            events: self.events.downgrade(),
        }
    }
    fn event(&self, event: Event) {
        let _ = self.events.send(event);
    }
}

/// Documents do not keep a dropped editor's worker alive.
pub(crate) struct WordIndexTrigger {
    events: mpsc::WeakUnboundedSender<Event>,
}
impl WordIndexTrigger {
    fn event(&self, event: Event) {
        if let Some(events) = self.events.upgrade() {
            let _ = events.send(event);
        }
    }
}

#[derive(Debug, Default)]
struct Pending {
    documents: HashMap<DocumentId, Option<Rope>>,
    reset: bool,
}
impl Pending {
    /// Control events and large changes bypass the typing debounce.
    fn push(&mut self, event: Event) -> bool {
        match event {
            Event::Open(id, text) => {
                self.documents.insert(id, Some(text));
                true
            }
            Event::Edit {
                document,
                text,
                significant,
            } => {
                self.documents.insert(document, Some(text));
                significant
            }
            Event::Close(id) => {
                self.documents.insert(id, None);
                true
            }
            Event::Reset => {
                self.documents.clear();
                self.reset = true;
                true
            }
        }
    }
}

const DEBOUNCE: Duration = Duration::from_secs(1);

async fn coordinate(
    index: WordIndex,
    mut events: mpsc::UnboundedReceiver<Event>,
    cancel: TaskHandle,
) {
    let mut indexed = Some(IndexedDocuments {
        index,
        documents: HashMap::default(),
    });
    let mut worker: Option<JoinHandle<IndexedDocuments>> = None;
    let mut pending = Pending::default();
    let mut deadline = None;
    let mut ready = false;
    loop {
        if cancel.is_canceled() {
            return;
        }
        if ready && worker.is_none() {
            let state = indexed.take().expect("idle worker owns its snapshots");
            let changes = std::mem::take(&mut pending);
            let cancel = cancel.clone();
            worker = Some(tokio::task::spawn_blocking(move || {
                state.apply(changes, &cancel)
            }));
            ready = false;
        }
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { return; };
                if pending.push(event) {
                    ready = true;
                    deadline = None;
                } else if !ready {
                    deadline = Some(Instant::now() + DEBOUNCE);
                }
            }
            result = async { worker.as_mut().unwrap().await }, if worker.is_some() => {
                indexed = Some(result.expect("word indexing worker panicked"));
                worker = None;
            }
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                deadline = None;
                ready = true;
            }
        }
    }
}

/// The worker alone owns the text represented by the published word counts.
struct IndexedDocuments {
    index: WordIndex,
    documents: HashMap<DocumentId, Rope>,
}
impl IndexedDocuments {
    fn apply(mut self, pending: Pending, cancel: &TaskHandle) -> Self {
        if cancel.is_canceled() {
            return self;
        }
        if pending.reset {
            self.index.inner.write().clear();
            self.documents.clear();
        }
        for (id, text) in pending.documents {
            if cancel.is_canceled() {
                break;
            }
            match (self.documents.remove(&id), text) {
                (Some(old), Some(text)) => {
                    if let Some((old_words, new_words)) =
                        word_index::changed_words(old.slice(..), text.slice(..), || {
                            cancel.is_canceled()
                        })
                    {
                        self.index
                            .inner
                            .write()
                            .replace_words(old_words, new_words, || cancel.is_canceled());
                    }
                    self.documents.insert(id, text);
                }
                (None, Some(text)) => {
                    self.index
                        .inner
                        .write()
                        .add_document(&text, || cancel.is_canceled());
                    self.documents.insert(id, text);
                }
                (Some(old), None) => self
                    .index
                    .inner
                    .write()
                    .remove_document(&old, || cancel.is_canceled()),
                (None, None) => {}
            }
        }
        self
    }
}

#[derive(Debug, Default, Clone)]
pub struct WordIndex {
    inner: Arc<RwLock<word_index::WordIndex>>,
    matcher: Arc<Mutex<fuzzy::Matcher>>,
}
impl WordIndex {
    pub fn matches(&self, pattern: &str) -> Vec<String> {
        self.inner.read().matches(pattern, &mut self.matcher.lock())
    }
}

/// Estimates whether a changeset is significant or small.
fn is_changeset_significant(changes: &ChangeSet) -> bool {
    use editor_core::Operation::*;

    let mut diff = 0;
    for operation in changes.changes() {
        match operation {
            Retain(_) => continue,
            Delete(_) | Insert(_) => diff += operation.len_chars(),
        }
    }

    // This is arbitrary and could be tuned further:
    diff > 1_000
}

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        let doc = doc!(event.editor, &event.doc);
        if doc.word_completion_enabled() {
            event
                .editor
                .handlers
                .word_index
                .event(Event::Open(doc.id(), doc.text().clone()));
        }
        Ok(())
    });

    register_hook!(move |event: &mut DocumentLanguageDidChange<'_>| {
        if let Some(doc) = event.editor.document(event.doc) {
            let handler = &event.editor.handlers.word_index;
            handler.event(if doc.word_completion_enabled() {
                Event::Open(doc.id(), doc.text().clone())
            } else {
                Event::Close(doc.id())
            });
        }
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if event.doc.word_completion_enabled()
            && let Some(trigger) = &event.doc.word_index_trigger
        {
            trigger.event(Event::Edit {
                document: event.doc.id(),
                text: event.doc.text().clone(),
                significant: is_changeset_significant(event.changes),
            });
        }
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        if let Some(trigger) = &event.doc.word_index_trigger {
            trigger.event(Event::Close(event.doc.id()));
        }
        Ok(())
    });

    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        if event.old.completion.word.enable != event.new.completion.word.enable {
            let handler = &event.editor.handlers.word_index;
            // Rebuild from current document settings, including language overrides.
            // Route the reset and inserts through the edit queue to preserve their order.
            handler.event(Event::Reset);
            for doc in event
                .editor
                .documents()
                .filter(|doc| doc.word_completion_enabled())
            {
                handler.event(Event::Open(doc.id(), doc.text().clone()));
            }
        }
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    impl WordIndex {
        fn counts(&self) -> std::collections::HashMap<String, u32> {
            self.inner
                .read()
                .word_counts()
                .map(|(word, count)| (word.to_owned(), count))
                .collect()
        }
    }

    async fn expect_words(handler: &Handler, expected: &[String]) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut actual = handler.index.matches("");
                actual.sort();
                if actual == expected {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("word index converges");
    }

    #[tokio::test]
    async fn batched_opens_resets_and_closes_are_lossless_without_yielding() {
        let handler = Handler::spawn();
        for i in 0..140 {
            handler.event(Event::Open(
                DocumentId::new(i + 1),
                Rope::from(format!("stale{i:03}")),
            ));
        }
        handler.event(Event::Reset);
        let expected: Vec<_> = (0..140).map(|i| format!("word{i:03}")).collect();
        for (i, word) in expected.iter().enumerate() {
            handler.event(Event::Open(
                DocumentId::new(i + 1),
                Rope::from(word.as_str()),
            ));
        }
        expect_words(&handler, &expected).await;
        for i in 0..140 {
            handler.event(Event::Close(DocumentId::new(i + 1)));
        }
        expect_words(&handler, &[]).await;
    }

    #[tokio::test]
    async fn typing_publishes_the_latest_snapshot_after_the_debounce() {
        let handler = Handler::spawn();
        let id = DocumentId::new(1);
        handler.event(Event::Open(id, Rope::from("original")));
        expect_words(&handler, &["original".into()]).await;
        handler.event(Event::Edit {
            document: id,
            text: Rope::from("intermediate"),
            significant: false,
        });
        handler.event(Event::Edit {
            document: id,
            text: Rope::from("latest"),
            significant: false,
        });
        expect_words(&handler, &["latest".into()]).await;
    }

    #[test]
    fn closing_a_document_removes_its_indexed_text_and_discards_pending_edits() {
        let mut controller = TaskController::new();
        let cancel = controller.restart();
        let index = WordIndex::default();
        let mut state = IndexedDocuments {
            index: index.clone(),
            documents: HashMap::default(),
        };
        let first = DocumentId::new(1);
        let second = DocumentId::new(2);
        let mut pending = Pending::default();
        pending.push(Event::Open(first, Rope::from("shared alpha")));
        pending.push(Event::Open(second, Rope::from("shared beta")));
        state = state.apply(pending, &cancel);
        assert_eq!(index.counts()["shared"], 2);
        let mut pending = Pending::default();
        pending.push(Event::Edit {
            document: first,
            text: Rope::from("pending gamma"),
            significant: false,
        });
        pending.push(Event::Close(first));
        state = state.apply(pending, &cancel);
        assert_eq!(
            index.counts(),
            std::collections::HashMap::from([("shared".into(), 1), ("beta".into(), 1)])
        );
        let mut pending = Pending::default();
        pending.push(Event::Open(first, Rope::from("shared delta")));
        state.apply(pending, &cancel);
        assert_eq!(index.counts()["shared"], 2);
        assert_eq!(index.counts()["delta"], 1);
    }

    quickcheck::quickcheck! {
        fn coalesced_snapshots_match_fresh_word_counts(events: Vec<(u8, Option<String>)>) -> bool {
            let mut controller = TaskController::new();
            let cancel = controller.restart();
            let index = WordIndex::default();
            let mut state = IndexedDocuments { index: index.clone(), documents: HashMap::default() };
            let mut expected = HashMap::default();
            let mut pending = Pending::default();
            for (key, text) in events {
                let id = DocumentId::new(usize::from(key % 4) + 1);
                match (key, text) {
                    (255, _) => { expected.clear(); pending.push(Event::Reset); }
                    (_, Some(text)) => {
                        let text = Rope::from(text);
                        expected.insert(id, text.clone());
                        pending.push(Event::Edit { document: id, text, significant: false });
                    }
                    (_, None) => { expected.remove(&id); pending.push(Event::Close(id)); }
                }
                if key % 3 == 0 { state = state.apply(std::mem::take(&mut pending), &cancel); }
            }
            state.apply(pending, &cancel);
            let mut fresh = word_index::WordIndex::default();
            for text in expected.values() { fresh.add_document(text, || false); }
            index.counts() == fresh.word_counts().map(|(word, count)| (word.to_owned(), count)).collect()
        }
    }
}

//! Indexing of words from open buffers.
//!
//! This provides an eventually consistent set of words used in any open buffers. This set is
//! later used for lexical completion.

use std::{sync::Arc, time::Duration};

use editor_core::{ChangeSet, Rope};
use event::{register_hook, TaskController, TaskHandle};
use foldhash::HashMap;
use parking_lot::{Mutex, RwLock};
use text_diff::compare_ropes;
use tokio::{sync::mpsc, time::Instant};

use crate::{
    events::{ConfigDidChange, DocumentDidChange, DocumentDidClose, DocumentDidOpen},
    DocumentId,
};

#[derive(Debug)]
struct Change {
    old_text: Rope,
    text: Rope,
    changes: ChangeSet,
    /// Whether `changes` is stale and must be recomputed from `old_text`/`text` before use.
    ///
    /// Set when this change is the result of coalescing several observed changes (see
    /// [`Hook::handle_event`]). The observed changesets cannot be reliably chained, so the
    /// coalesced changeset is recomputed once, lazily, at [`Hook::finish_debounce`].
    dirty: bool,
}

#[derive(Debug)]
enum Event {
    Insert(Rope),
    Update(DocumentId, Change),
    Delete(DocumentId, Rope),
    /// Clear the entire word index.
    /// This is used to clear memory when the feature is turned off.
    Clear,
}

/// Sends an event to the coordinator task (lossy).
///
/// The coordinator stops when its [`Handler`] is dropped. A closed channel means that index is no
/// longer in use, so dropping the event is harmless.
fn send(coordinator: &mpsc::UnboundedSender<Event>, event: Event) {
    let _ = coordinator.send(event);
}

#[derive(Debug)]
pub struct Handler {
    index: WordIndex,
    /// A sender into an async hook which debounces updates to the index.
    hook: mpsc::UnboundedSender<Event>,
    /// Cancels in-flight indexing when the handler is dropped.
    ///
    /// Indexing a large document runs on a blocking task which cannot be preempted. Without this,
    /// dropping the tokio runtime on shutdown would block until that task finishes, keeping the
    /// process alive and unresponsive. The indexing task holds a [TaskHandle] from this
    /// controller and checks it periodically.
    _cancel: TaskController,
}

impl Handler {
    pub(crate) fn index(&self) -> &WordIndex {
        &self.index
    }

    pub fn spawn() -> Self {
        let index = WordIndex::default();
        let (tx, rx) = mpsc::unbounded_channel();
        let mut cancel = TaskController::new();
        tokio::spawn(index.clone().run(rx, cancel.restart()));
        // Opening documents and rebuilding configuration must not drop events when
        // a frontend processes a batch without yielding. Debounce all events in order.
        let (hook, events) = mpsc::unbounded_channel();
        tokio::spawn(
            Hook {
                changes: HashMap::default(),
                coordinator: tx,
            }
            .run(events),
        );
        Self {
            hook,
            index,
            _cancel: cancel,
        }
    }

    pub(crate) fn document_trigger(&self) -> WordIndexTrigger {
        WordIndexTrigger {
            events: self.hook.downgrade(),
        }
    }

    fn event(&self, event: Event) {
        send(&self.hook, event);
    }
}

/// Documents can schedule indexing without keeping a dropped editor's worker alive.
pub(crate) struct WordIndexTrigger {
    events: mpsc::WeakUnboundedSender<Event>,
}

impl WordIndexTrigger {
    fn event(&self, event: Event) {
        if let Some(events) = self.events.upgrade() {
            send(&events, event);
        }
    }
}

#[derive(Debug)]
struct Hook {
    changes: HashMap<DocumentId, Change>,
    coordinator: mpsc::UnboundedSender<Event>,
}

const DEBOUNCE: Duration = Duration::from_secs(1);

impl Hook {
    async fn run(mut self, mut events: mpsc::UnboundedReceiver<Event>) {
        let mut deadline = None;
        loop {
            let event = match deadline {
                Some(at) => match tokio::time::timeout_at(at, events.recv()).await {
                    Ok(event) => event,
                    Err(_) => {
                        self.finish_debounce();
                        deadline = None;
                        continue;
                    }
                },
                None => events.recv().await,
            };
            let Some(event) = event else {
                return;
            };
            deadline = self.handle_event(event, deadline);
        }
    }

    fn handle_event(&mut self, event: Event, timeout: Option<Instant>) -> Option<Instant> {
        match event {
            Event::Insert(text) => {
                send(&self.coordinator, Event::Insert(text));
                timeout
            }
            Event::Update(doc, change) => {
                if let Some(pending_change) = self.changes.get_mut(&doc) {
                    // There is already a change waiting for this document. Coalesce: keep the
                    // original `old_text` and advance to the latest `text`.
                    //
                    // We deliberately do NOT chain the observed changesets here. The index skips
                    // ghost transactions (see `register_hooks`), so a ghost edit may have altered
                    // the document between these two observed changes, which may leave them
                    // non-contiguous in ways a cheap check can't catch. Instead, mark the change
                    // dirty and recompute the changeset from the real ropes at `finish_debounce`,
                    // i.e.  once per debounce window rather than once per keystroke.
                    pending_change.text = change.text;
                    pending_change.dirty = true;
                    Some(Instant::now() + DEBOUNCE)
                } else if !is_changeset_significant(&change.changes) {
                    // The change is small: debounce so a burst of edits coalesces before the
                    // index updates.
                    self.changes.insert(doc, change);
                    Some(Instant::now() + DEBOUNCE)
                } else {
                    // The change is large: update the index immediately rather than waiting out
                    // the debounce.
                    send(&self.coordinator, Event::Update(doc, change));
                    timeout
                }
            }
            Event::Delete(doc, text) => {
                // If there are pending changes that haven't been indexed since the last debounce,
                // forget them and delete the old text.
                match self.changes.remove(&doc) {
                    Some(change) => {
                        send(&self.coordinator, Event::Delete(doc, change.old_text));
                    }
                    _ => {
                        send(&self.coordinator, Event::Delete(doc, text));
                    }
                }
                timeout
            }
            Event::Clear => {
                // A reset must also discard edits still waiting for their debounce deadline.
                self.changes.clear();
                send(&self.coordinator, Event::Clear);
                None
            }
        }
    }

    fn finish_debounce(&mut self) {
        for (doc, mut change) in self.changes.drain() {
            // A coalesced change carries a stale changeset; recompute it from the real endpoints.
            // This diff is valid regardless of any ghost edits skipped between observed changes.
            if change.dirty {
                change.changes = compare_ropes(&change.old_text, &change.text)
                    .changes()
                    .clone();
                change.dirty = false;
            }
            send(&self.coordinator, Event::Update(doc, change));
        }
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

    fn add_document(&self, text: &Rope, cancel: &TaskHandle) {
        self.inner
            .write()
            .add_document(text, || cancel.is_canceled());
    }

    fn update_document(&self, old: &Rope, text: &Rope, changes: &ChangeSet, cancel: &TaskHandle) {
        self.inner
            .write()
            .update_document(old, text, changes, || cancel.is_canceled());
    }

    fn remove_document(&self, text: &Rope, cancel: &TaskHandle) {
        self.inner
            .write()
            .remove_document(text, || cancel.is_canceled());
    }

    fn clear(&self) {
        self.inner.write().clear();
    }

    /// Coordinate the indexing of documents.
    ///
    /// This task wraps a MPSC queue and spawns blocking tasks which update the index. Updates
    /// are applied one-by-one to ensure that changes to the index are **serialized**:
    /// updates to each document must be applied in-order.
    async fn run(self, mut events: mpsc::UnboundedReceiver<Event>, cancel: TaskHandle) {
        while let Some(event) = events.recv().await {
            if cancel.is_canceled() {
                return;
            }
            let this = self.clone();
            let cancel = cancel.clone();
            tokio::task::spawn_blocking(move || match event {
                Event::Insert(text) => {
                    this.add_document(&text, &cancel);
                }
                Event::Update(
                    _doc,
                    Change {
                        old_text,
                        text,
                        changes,
                        ..
                    },
                ) => {
                    this.update_document(&old_text, &text, &changes, &cancel);
                }
                Event::Delete(_doc, text) => {
                    this.remove_document(&text, &cancel);
                }
                Event::Clear => {
                    this.clear();
                }
            })
            .await
            .unwrap();
        }
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
                .event(Event::Insert(doc.text().clone()));
        }
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if !event.ghost_transaction
            && event.doc.word_completion_enabled()
            && let Some(trigger) = &event.doc.word_index_trigger
        {
            trigger.event(Event::Update(
                event.doc.id(),
                Change {
                    old_text: event.old_text.clone(),
                    text: event.doc.text().clone(),
                    changes: event.changes.clone(),
                    dirty: false,
                },
            ));
        }
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        if event.doc.word_completion_enabled()
            && let Some(trigger) = &event.doc.word_index_trigger
        {
            trigger.event(Event::Delete(event.doc.id(), event.doc.text().clone()));
        }
        Ok(())
    });

    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        if event.old.completion.word.enable != event.new.completion.word.enable {
            let handler = &event.editor.handlers.word_index;
            // Rebuild from current document settings, including language overrides.
            // Route the reset and inserts through the edit queue to preserve their order.
            handler.event(Event::Clear);
            for doc in event
                .editor
                .documents()
                .filter(|doc| doc.word_completion_enabled())
            {
                handler.event(Event::Insert(doc.text().clone()));
            }
        }
        Ok(())
    });
}

#[cfg(test)]
mod tests {

    use super::*;
    use quickcheck::{Arbitrary, Gen};

    impl WordIndex {
        fn counts(&self) -> std::collections::HashMap<String, u32> {
            self.inner
                .read()
                .word_counts()
                .map(|(word, count)| (word.to_owned(), count))
                .collect()
        }
    }

    #[tokio::test]
    async fn batched_control_events_are_lossless_without_yielding() {
        let handler = Handler::spawn();
        for i in 0..140 {
            handler.event(Event::Insert(Rope::from_str(&format!("stale{i:03}"))));
        }
        handler.event(Event::Clear);
        let expected: Vec<_> = (0..140).map(|i| format!("word{i:03}")).collect();
        for word in &expected {
            handler.event(Event::Insert(Rope::from_str(word)));
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut words = handler.index.matches("");
                words.sort();
                if words == expected {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for (i, word) in expected.iter().enumerate() {
            handler.event(Event::Delete(DocumentId::new(i + 1), Rope::from_str(word)));
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            while !handler.index.matches("").is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    fn build_change(old: &str, new: &str) -> Change {
        let old_text = Rope::from_str(old);
        let text = Rope::from_str(new);
        let changes = compare_ropes(&old_text, &text).changes().clone();
        Change {
            old_text,
            text,
            changes,
            dirty: false,
        }
    }

    /// Drives a [`Hook`] through a sequence of observed `(old_text, new_text)` changes to a single
    /// document, flushes the debounce, and returns the change the hook hands to the indexer (with
    /// its changeset recomputed if the observed changes were coalesced). Returns `None` if no
    /// change was emitted.
    fn coalesce<'a>(observations: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<Change> {
        let (coordinator, mut rx) = mpsc::unbounded_channel();
        let mut hook = Hook {
            changes: HashMap::default(),
            coordinator,
        };
        let doc = DocumentId::default();
        for (old, new) in observations {
            hook.handle_event(Event::Update(doc, build_change(old, new)), None);
        }
        hook.finish_debounce();
        match rx.try_recv() {
            Ok(Event::Update(_, change)) => Some(change),
            _ => None,
        }
    }

    #[track_caller]
    fn assert_coalesces(observations: &[(&str, &str)]) {
        let change = coalesce(observations.iter().copied())
            .expect("a coalesced change should have been emitted");
        let first = observations.first().unwrap();
        let last = observations.last().unwrap();

        // The coalesced change spans from the first text observed to the last.
        assert_eq!(change.old_text, Rope::from_str(first.0));
        assert_eq!(change.text, Rope::from_str(last.1));

        // Its changeset must faithfully map `old_text` -> `text`; `update_document` relies on this
        // to locate the windows it reindexes.
        let mut applied = change.old_text.clone();
        assert!(change.changes.apply(&mut applied));
        assert_eq!(applied, change.text);

        // Reindexing must converge onto exactly the word multiset of the final text (words and
        // refcounts) matching a fresh index built from scratch.
        let mut cancel = TaskController::new();
        let handle = cancel.restart();
        let index = WordIndex::default();
        index.add_document(&change.old_text, &handle);
        index.update_document(&change.old_text, &change.text, &change.changes, &handle);

        let fresh = WordIndex::default();
        fresh.add_document(&change.text, &handle);

        assert_eq!(index.counts(), fresh.counts());
    }

    #[test]
    fn hook_coalesces_contiguous_changes() {
        // Two real edits that chain cleanly: the text after the first edit is the text before the
        // second, so the changesets compose directly.
        assert_coalesces(&[
            ("the quick brown fox", "the slowish brown fox"),
            ("the slowish brown fox", "the slowish green fox"),
        ]);
    }

    #[test]
    fn hook_coalesces_across_skipped_ghost_edit() {
        // A ghost transaction edited the document between the two real edits and was not reported
        // to the index, so the text before the second edit no longer matches the text after the
        // first. Here it is longer, which would make a chained `ChangeSet::compose` panic on its
        // `len_after == len` precondition.
        assert_coalesces(&[
            ("the quick brown fox", "the quick brown foxes"),
            ("the quick brown foxes jumped over", "the lazy brown foxes"),
        ]);
    }

    #[test]
    fn hook_coalesces_across_length_preserving_ghost_edit() {
        // The subtle case: a ghost edit changes content without changing length, in a region the
        // next real edit leaves untouched. The two observed changes then have matching lengths
        // but different contents, so a length check alone cannot tell they are non-contiguous.
        let p = ".".repeat(60);
        assert_coalesces(&[
            (&format!("aaa{p}ggg"), &format!("bbb{p}ggg")),
            (&format!("bbb{p}hhh"), &format!("ccc{p}hhh")),
        ]);
    }

    fn sample_text(g: &mut Gen) -> String {
        g.choose(&[
            "the quick brown fox",
            "one two three",
            "αβγ café",
            "",
            "word\nnext",
            "ábc xyz",
        ])
        .unwrap()
        .to_string()
    }

    fn random_edit(g: &mut Gen, text: &str) -> String {
        let chars: Vec<_> = text.chars().collect();
        let start = usize::arbitrary(g) % (chars.len() + 1);
        let end = start + usize::arbitrary(g) % (chars.len() - start + 1);
        let mut result = chars[..start].iter().collect::<String>();
        result.push_str(&sample_text(g));
        result.extend(&chars[end..]);
        result
    }

    /// A sequence of `(old_text, new_text)` changes as the hook observes them. Some steps leave
    /// a gap (the next change's `old_text` differs from the previous `new_text`) modelling an
    /// unobserved ghost edit applied to the document between two real ones.
    #[derive(Clone, Debug)]
    struct Observations(Vec<(String, String)>);

    impl Arbitrary for Observations {
        fn arbitrary(g: &mut Gen) -> Self {
            let steps = usize::arbitrary(g) % 5 + 1;
            let mut current = sample_text(g);
            let mut observations = Vec::with_capacity(steps);
            for _ in 0..steps {
                let observed_old = if bool::arbitrary(g) {
                    // A ghost edit perturbed the document before this real edit, so the observed
                    // `old_text` no longer matches the previous `new_text`.
                    random_edit(g, &current)
                } else {
                    current.clone()
                };
                let new = random_edit(g, &observed_old);
                observations.push((observed_old, new.clone()));
                current = new;
            }
            Observations(observations)
        }
        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            Box::new(self.0.shrink().map(Observations))
        }
    }

    quickcheck::quickcheck! {
        /// Coalescing a sequence of observed changes (gaps and all) and flushing yields a change
        /// that maps its `old_text` to its `text` and drives the index to the same multiset as a
        /// fresh index of the final text.
        fn prop_hook_coalescing_matches_fresh(obs: Observations) -> bool {
            let Some(change) = coalesce(obs.0.iter().map(|(o, n)| (o.as_str(), n.as_str()))) else {
                return true;
            };

            let mut applied = change.old_text.clone();
            if !change.changes.apply(&mut applied) || applied != change.text {
                return false;
            }

            let mut cancel = TaskController::new();
            let handle = cancel.restart();
            let incremental = WordIndex::default();
            incremental.add_document(&change.old_text, &handle);
            incremental.update_document(&change.old_text, &change.text, &change.changes, &handle);

            let fresh = WordIndex::default();
            fresh.add_document(&change.text, &handle);

            incremental.counts() == fresh.counts()
        }
    }
}

//! Invalidation of live reviews belongs to the editor, independently of views.

use std::{
    collections::HashSet,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
    time::Duration,
};

use event::{register_hook, send_blocking, AsyncHook, TaskController};
use tokio::{
    sync::{mpsc::Sender, Semaphore},
    time::Instant,
};

use crate::{
    callbacks::EditorCallbackSender,
    events::{
        ConfigDidChange, DocumentDidChange, DocumentDidClose, DocumentDidOpen,
        DocumentLanguageDidChange,
    },
    file_watcher::{EventType, Events},
    DocumentId, Editor,
};

pub(crate) struct Handler {
    pub(super) hunk: parking_lot::Mutex<TaskController>,
    pub(super) worker: Arc<Semaphore>,
    events: Sender<RefreshEvent>,
    active: Arc<AtomicBool>,
    callbacks: Arc<EditorCallbackSender>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum RefreshEvent {
    Source(DocumentId),
    Review(DocumentId),
}

impl Handler {
    pub(crate) fn new(callbacks: EditorCallbackSender) -> Self {
        let callbacks = Arc::new(callbacks);
        let events = Debounce {
            callbacks: Arc::downgrade(&callbacks),
            pending: HashSet::new(),
        }
        .spawn();
        Self {
            hunk: Default::default(),
            worker: Arc::new(Semaphore::new(1)),
            events,
            callbacks,
            active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn document_trigger(&self) -> ReviewTrigger {
        ReviewTrigger {
            events: self.events.clone(),
            active: Arc::downgrade(&self.active),
        }
    }
}

pub(crate) struct ReviewTrigger {
    events: Sender<RefreshEvent>,
    active: Weak<AtomicBool>,
}

impl ReviewTrigger {
    fn changed(&self, doc: DocumentId) {
        if self
            .active
            .upgrade()
            .is_some_and(|active| active.load(Ordering::Relaxed))
        {
            send_blocking(&self.events, RefreshEvent::Source(doc));
        }
    }
}

pub(crate) fn configure(editor: &Editor) {
    editor
        .handlers
        .change_review
        .active
        .store(has_live_reviews(editor), Ordering::Relaxed);
    crate::features::auto_reload::configure(editor);
}

struct Debounce {
    callbacks: Weak<EditorCallbackSender>,
    pending: HashSet<RefreshEvent>,
}

impl AsyncHook for Debounce {
    type Event = RefreshEvent;

    fn handle_event(&mut self, event: RefreshEvent, _: Option<Instant>) -> Option<Instant> {
        self.pending.insert(event);
        Some(Instant::now() + Duration::from_millis(250))
    }

    fn finish_debounce(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        let Some(callbacks) = self.callbacks.upgrade() else {
            return;
        };
        let owner = self.callbacks.clone();
        callbacks.send_blocking(move |editor| {
            if !owner.ptr_eq(&Arc::downgrade(&editor.handlers.change_review.callbacks)) {
                return;
            }
            let mut reviews = HashSet::new();
            for event in pending {
                match event {
                    RefreshEvent::Review(id) => {
                        reviews.insert(id);
                    }
                    RefreshEvent::Source(id) => {
                        if let Some(path) = editor.document(id).and_then(|doc| doc.path()) {
                            reviews.extend(
                                editor
                                    .documents()
                                    .filter(|doc| {
                                        doc.review().is_some_and(|review| {
                                            review.is_live() && review.path() == path
                                        }) || doc.project_review().is_some_and(|review| {
                                            review.comparison == super::ReviewComparison::Buffer
                                                && review.source_documents.contains(&id)
                                        })
                                    })
                                    .map(|doc| doc.id()),
                            );
                        }
                    }
                }
            }
            for id in reviews {
                if editor
                    .document(id)
                    .is_some_and(|doc| doc.project_review().is_some())
                {
                    #[cfg(feature = "git")]
                    if editor.handlers().git_operations.busy() {
                        queue(editor, id);
                        continue;
                    }
                    editor.start_project_review(id);
                } else {
                    editor.refresh_scheduled_review(id);
                }
            }
        });
    }
}

// Lifecycle events also refresh buffer reviews for newly opened lexical aliases;
// their repository membership is resolved off-thread by project loading.
fn for_path<'a>(editor: &'a Editor, path: &Path) -> impl Iterator<Item = DocumentId> + 'a {
    // Own the path because callers can obtain it from a document that is closing.
    let path = path.to_owned();
    editor.documents().filter_map(move |doc| {
        (doc.review()
            .is_some_and(|review| review.is_live() && review.path() == path)
            || doc
                .project_review()
                .is_some_and(|review| review.comparison == super::ReviewComparison::Buffer))
        .then_some(doc.id())
    })
}

pub(super) fn queue_path(editor: &Editor, path: &Path) {
    for id in for_path(editor, path) {
        queue(editor, id);
    }
}

pub(crate) fn queue(editor: &Editor, id: DocumentId) {
    send_blocking(
        &editor.handlers.change_review.events,
        RefreshEvent::Review(id),
    );
}

pub(crate) fn has_live_reviews(editor: &Editor) -> bool {
    editor.documents().any(|doc| {
        doc.project_review().is_some() || doc.review().is_some_and(|review| review.is_live())
    })
}

/// Polling also covers ignored files and repositories outside native watch roots.
/// The worker compares metadata first, retaining unchanged models and their syntax.
pub(crate) fn poll(editor: &mut Editor) {
    let ids: Vec<_> = editor
        .documents()
        .filter_map(|doc| {
            doc.review()
                .filter(|review| review.is_live() && !review.is_refreshing())
                .map(|_| doc.id())
        })
        .collect();
    for id in ids {
        editor.refresh_scheduled_review(id);
    }
}

pub(crate) fn invalidate(editor: &Editor) {
    for doc in editor.documents() {
        if doc.project_review().is_some() || doc.review().is_some_and(|review| review.is_live()) {
            queue(editor, doc.id());
        }
    }
}

pub(crate) fn handle_file_events(editor: &Editor, events: &Events) {
    for doc in editor.documents() {
        if doc.project_review().is_some_and(|review| {
            events.iter().any(|event| {
                event.ty != EventType::Tempfile
                    && (event.path.as_std_path().starts_with(&review.root)
                        || editor.file_watcher.is_vcs_path(event.path.as_std_path()))
            })
        }) || doc.review().is_some_and(|review| {
            review.is_live()
                && events.iter().any(|event| {
                    event.ty != EventType::Tempfile && review.affected_by(event.path.as_std_path())
                })
        }) {
            queue(editor, doc.id());
        }
    }
}

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if let Some(trigger) = &event.doc.review_trigger {
            trigger.changed(event.doc.id());
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        if let Some(path) = event.editor.document(event.doc).and_then(|doc| doc.path()) {
            queue_path(event.editor, path);
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        if let Some(path) = event.doc.path() {
            queue_path(event.editor, path);
        }
        configure(event.editor);
        Ok(())
    });
    register_hook!(move |event: &mut DocumentLanguageDidChange<'_>| {
        if let Some(path) = event.editor.document(event.doc).and_then(|doc| doc.path()) {
            queue_path(event.editor, path);
        }
        Ok(())
    });
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        for doc in event.editor.documents() {
            if doc.review().is_some() {
                queue(event.editor, doc.id());
            }
        }
        Ok(())
    });
}

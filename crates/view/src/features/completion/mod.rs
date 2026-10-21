//! Editor-owned completion requests, previews, and acceptance. Frontends own their menus.

use crate::{
    callbacks::EditorCallbackSender,
    document::{Mode, SavePoint},
    events::{ConfigDidChange, DocumentDidClose, DocumentFocusLost, LanguageServerExited},
    DocumentId, Editor, ViewId,
};
use editor_config::Config;
use editor_core::{chars::char_is_word, Uri};
use event::{register_hook, send_blocking, AsyncHook, TaskController, TaskHandle};
use language_config::LanguageServerFeature;
use lsp_client::lsp;
use parking_lot::Mutex;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Weak,
    },
    time::Duration,
};
use stdx::rope::RopeExt as _;
use stdx::rope::RopeSliceExt;
use tokio::{sync::mpsc::Sender, task::JoinSet};

pub(crate) use acceptance::AcceptedCompletions;
use acceptance::{additional_edits_transaction, AcceptedCompletion};
pub use edit::CompletionApply;
use edit::CompletionEdit;
use item::CompletionItems;
pub use item::{
    CompletionItem, CompletionProvider, CompletionResponse, LspCompletionItem, TextCompletion,
};
pub use request::request_incomplete_completion_list;
use request::{Debounce, Trigger, TriggerKind};
pub use resolve::ResolveHandler;

mod acceptance;
mod edit;
mod item;
mod path;
mod request;
mod resolve;
mod word;

struct Shared {
    callbacks: EditorCallbackSender,
    epoch: AtomicU64,
    trigger: Mutex<Option<Trigger>>,
    timeout: Mutex<Duration>,
}

pub struct CompletionHandler {
    shared: Arc<Shared>,
    event_tx: Sender<(CompletionEvent, u64)>,
    pub active_completions: HashMap<CompletionProvider, ResponseContext>,
    request_controller: TaskController,
    session_controller: TaskController,
    session: Option<CompletionSession>,
    edit: Option<CompletionEdit>,
    updates: VecDeque<CompletionUpdate>,
}

impl CompletionHandler {
    pub fn new(callbacks: EditorCallbackSender, config: &Config) -> Self {
        let shared = Arc::new(Shared {
            callbacks,
            epoch: AtomicU64::new(0),
            trigger: Mutex::new(None),
            timeout: Mutex::new(config.completion.trigger_delay),
        });
        let event_tx = Debounce::new(Arc::downgrade(&shared)).spawn();
        Self {
            shared,
            event_tx,
            active_completions: HashMap::new(),
            request_controller: TaskController::new(),
            session_controller: TaskController::new(),
            session: None,
            edit: None,
            updates: VecDeque::new(),
        }
    }

    pub fn event(&self, event: CompletionEvent) {
        let mut trigger = self.shared.trigger.lock();
        let invalidate = match &event {
            CompletionEvent::Cancel => {
                *trigger = None;
                true
            }
            CompletionEvent::ManualTrigger { cursor, doc, view }
            | CompletionEvent::TriggerChar { cursor, doc, view } => {
                *trigger = Some(Trigger {
                    pos: *cursor,
                    doc: *doc,
                    view: *view,
                    kind: if matches!(&event, CompletionEvent::ManualTrigger { .. }) {
                        TriggerKind::Manual
                    } else {
                        TriggerKind::TriggerChar
                    },
                });
                true
            }
            CompletionEvent::AutoTrigger { cursor, doc, view } => {
                let changed =
                    trigger.is_none_or(|trigger| trigger.doc != *doc || trigger.view != *view);
                if changed {
                    *trigger = Some(Trigger {
                        pos: *cursor,
                        doc: *doc,
                        view: *view,
                        kind: TriggerKind::Auto,
                    });
                }
                changed
            }
            CompletionEvent::DeleteText { cursor } => {
                let changed = trigger.is_some_and(|trigger| *cursor < trigger.pos);
                if changed {
                    *trigger = None;
                }
                changed
            }
        };
        if invalidate {
            self.shared.epoch.fetch_add(1, Ordering::Relaxed);
        }
        let epoch = self.shared.epoch.load(Ordering::Relaxed);
        drop(trigger);
        send_blocking(&self.event_tx, (event, epoch));
    }

    /// Dismiss the displayed session without canceling a newly scheduled trigger.
    pub fn dismiss(&mut self) {
        self.session_controller.cancel();
        self.request_controller.cancel();
        self.session = None;
        self.active_completions.clear();
    }

    /// Cancel pending and displayed work and notify the frontend to dismiss its menu.
    pub(crate) fn invalidate(&mut self) {
        if matches!(self.edit, Some(CompletionEdit::Preview { .. })) {
            self.edit = None;
        }
        self.event(CompletionEvent::Cancel);
        self.dismiss();
        self.updates.clear();
        self.updates.push_back(CompletionUpdate {
            kind: UpdateKind::Hide {
                owner: Arc::downgrade(&self.shared),
                epoch: self.shared.epoch.load(Ordering::Relaxed),
            },
        });
        self.shared.callbacks.redraw().request();
    }
}

fn belongs_to(owner: &Weak<Shared>, editor: &Editor, epoch: u64) -> bool {
    owner.ptr_eq(&Arc::downgrade(&editor.handlers.completions.shared))
        && editor
            .handlers
            .completions
            .shared
            .epoch
            .load(Ordering::Relaxed)
            == epoch
}

/// A completion session deliberately survives typing: its savepoints
/// map provider edits back to the request snapshot. Explicit cancellation ends that lifetime.
/// Frontends retain the opaque token delivered with their menu to reject stale input.
#[derive(Clone)]
pub struct CompletionSession {
    owner: Weak<Shared>,
    epoch: u64,
    cancel: TaskHandle,
    trigger: Trigger,
    uri: Option<Uri>,
}

impl CompletionSession {
    fn new(editor: &mut Editor, trigger: Trigger, epoch: u64) -> Self {
        let uri = editor.document(trigger.doc).and_then(|doc| doc.uri());
        let handler = &mut editor.handlers.completions;
        *handler.shared.trigger.lock() = Some(trigger);
        Self {
            owner: Arc::downgrade(&handler.shared),
            epoch,
            cancel: handler.session_controller.restart(),
            trigger,
            uri,
        }
    }

    fn is_current(&self, editor: &Editor) -> bool {
        !self.cancel.is_canceled()
            && belongs_to(&self.owner, editor, self.epoch)
            && editor.mode() == Mode::Insert
            && (self.trigger.kind == TriggerKind::Manual || editor.config().completion.auto_trigger)
            && editor.tree.focus() == self.trigger.view
            && editor
                .tree
                .try_get(self.trigger.view)
                .is_some_and(|view| view.doc == self.trigger.doc)
            && editor.document(self.trigger.doc).is_some_and(|doc| {
                doc.uri() == self.uri
                    && doc
                        .try_selection(self.trigger.view)
                        .is_some_and(|selection| {
                            selection.primary().cursor(doc.text().slice(..)) >= self.trigger.pos
                        })
            })
    }

    fn supports(&self, editor: &Editor, provider: CompletionProvider) -> bool {
        let Some(doc) = editor.document(self.trigger.doc) else {
            return false;
        };
        match provider {
            CompletionProvider::Lsp(id) => {
                editor.config().lsp.enable
                    && editor.language_server_by_id(id).is_some()
                    && doc
                        .language_servers_with_feature(
                            language_config::LanguageServerFeature::Completion,
                        )
                        .any(|server| server.id() == id)
            }
            CompletionProvider::Path => doc.path_completion_enabled(),
            CompletionProvider::Word => doc.word_completion_enabled(),
        }
    }
}

/// Editor updates that have not yet been applied by a frontend.
pub struct CompletionUpdate {
    kind: UpdateKind,
}

enum UpdateKind {
    Filter {
        session: CompletionSession,
        character: Option<char>,
    },
    Start {
        owner: Weak<Shared>,
        epoch: u64,
        trigger: Trigger,
        handle: TaskHandle,
    },
    Result {
        session: CompletionSession,
        request: Option<TaskHandle>,
        payload: Payload,
    },
    Hide {
        owner: Weak<Shared>,
        epoch: u64,
    },
}

enum Payload {
    Show {
        items: Vec<CompletionItem>,
        context: HashMap<CompletionProvider, ResponseContext>,
        trigger: Trigger,
    },
    Provider {
        response: CompletionResponse,
        is_incomplete: bool,
    },
    Resolved {
        old: Arc<LspCompletionItem>,
        item: Box<CompletionItem>,
    },
}

/// Presentation work remaining after shared state and lifetime validation.
pub enum CompletionChange {
    /// Refine the displayed menu after accepted input; None removes one character.
    Filter {
        character: Option<char>,
    },
    /// Record a request in frontend input history at this point, before processing more input.
    Started,
    Show {
        items: Vec<CompletionItem>,
        session: CompletionSession,
    },
    Provider {
        response: CompletionResponse,
        is_incomplete: bool,
    },
    Resolved {
        old: Arc<LspCompletionItem>,
        item: Box<CompletionItem>,
    },
    Hide,
}

impl std::fmt::Debug for CompletionUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompletionUpdate").finish_non_exhaustive()
    }
}

impl CompletionUpdate {
    fn start(owner: Weak<Shared>, epoch: u64, trigger: Trigger, handle: TaskHandle) -> Self {
        Self {
            kind: UpdateKind::Start {
                owner,
                epoch,
                trigger,
                handle,
            },
        }
    }

    pub fn apply(self, editor: &mut Editor) -> Option<CompletionChange> {
        match self.kind {
            UpdateKind::Filter { session, character } => (!session.cancel.is_canceled()
                && belongs_to(&session.owner, editor, session.epoch)
                && editor.mode() == Mode::Insert
                && editor.tree.focus() == session.trigger.view
                && editor
                    .tree
                    .try_get(session.trigger.view)
                    .is_some_and(|view| view.doc == session.trigger.doc)
                && is_shown(editor))
            .then_some(CompletionChange::Filter { character }),
            UpdateKind::Start {
                owner,
                epoch,
                trigger,
                handle,
            } => {
                if !belongs_to(&owner, editor, epoch) {
                    return None;
                }
                request::request_completions(trigger, handle, editor, epoch)
                    .then_some(CompletionChange::Started)
            }
            UpdateKind::Hide { owner, epoch } => {
                belongs_to(&owner, editor, epoch).then_some(CompletionChange::Hide)
            }
            UpdateKind::Result {
                session,
                request,
                payload,
            } => {
                if !session.is_current(editor)
                    || request.is_some_and(|request| request.is_canceled())
                {
                    return None;
                }
                match payload {
                    Payload::Show {
                        mut items,
                        mut context,
                        trigger,
                    } => {
                        if is_shown(editor) {
                            return None;
                        }
                        items.retain(|item| session.supports(editor, item.provider()));
                        context.retain(|&provider, _| session.supports(editor, provider));
                        word::retain_valid_completions(
                            trigger,
                            editor.document(trigger.doc).unwrap(),
                            trigger.view,
                            &mut items,
                        );
                        editor.handlers.completions.active_completions = context;
                        Some(CompletionChange::Show { items, session })
                    }
                    Payload::Provider {
                        response,
                        is_incomplete,
                    } => {
                        if !is_shown(editor) || !session.supports(editor, response.provider()) {
                            return None;
                        }
                        editor
                            .handlers
                            .completions
                            .active_completions
                            .insert(response.provider(), response.context.clone());
                        Some(CompletionChange::Provider {
                            response,
                            is_incomplete,
                        })
                    }
                    Payload::Resolved { old, item } => {
                        if !is_shown(editor) || !session.supports(editor, item.provider()) {
                            return None;
                        }
                        Some(CompletionChange::Resolved { old, item })
                    }
                }
            }
        }
    }
}

/// Whether a completion menu has been opened and has not yet been finished.
pub(crate) fn input_applied(editor: &mut Editor, kind: crate::editor::InputKind) {
    use crate::editor::InputKind;
    if editor.mode() != Mode::Insert {
        return;
    }
    match kind {
        InputKind::Character(character) if is_shown(editor) => {
            filter_input(editor, Some(character))
        }
        InputKind::Backspace if is_shown(editor) => filter_input(editor, None),
        InputKind::Character(_) => trigger_auto_completion(editor, false),
        InputKind::Backspace | InputKind::Delete => {
            let (view, doc) = current_ref!(editor);
            let cursor = doc
                .selection(view.id)
                .primary()
                .cursor(doc.text().slice(..));
            editor
                .handlers
                .completions
                .event(CompletionEvent::DeleteText { cursor });
        }
        InputKind::Other => invalidate(editor),
    }
}

fn filter_input(editor: &mut Editor, character: Option<char>) {
    if let Some(session) = editor.handlers.completions.session.clone() {
        editor
            .handlers
            .completions
            .updates
            .push_back(CompletionUpdate {
                kind: UpdateKind::Filter { session, character },
            });
        editor.redraw().request();
    }
}

pub fn is_shown(editor: &Editor) -> bool {
    editor.handlers.completions.edit.is_some()
}

/// Whether the menu has an unaccepted presentation preview.
pub fn has_preview(editor: &Editor) -> bool {
    matches!(
        editor.handlers.completions.edit,
        Some(CompletionEdit::Preview { .. })
    )
}

/// Whether the displayed completion session can still be used by the frontend.
pub fn is_active(editor: &Editor) -> bool {
    editor
        .handlers
        .completions
        .session
        .as_ref()
        .is_some_and(|session| session.is_current(editor))
}

pub fn next_update(editor: &mut Editor) -> Option<CompletionUpdate> {
    editor.handlers.completions.updates.pop_front()
}

fn dispatch(owner: &Weak<Shared>, callback: impl FnOnce(&mut Editor) + Send + 'static) {
    if let Some(shared) = owner.upgrade() {
        shared.callbacks.send_blocking(callback);
    }
}

async fn deliver(session: CompletionSession, request: Option<TaskHandle>, payload: Payload) {
    let Some(shared) = session.owner.upgrade() else {
        return;
    };
    let callbacks = shared.callbacks.clone();
    drop(shared);
    callbacks
        .send(move |editor| {
            if session.is_current(editor)
                && request
                    .as_ref()
                    .is_none_or(|request| !request.is_canceled())
            {
                editor
                    .handlers
                    .completions
                    .updates
                    .push_back(CompletionUpdate {
                        kind: UpdateKind::Result {
                            session,
                            request,
                            payload,
                        },
                    });
                editor.redraw().request();
            }
        })
        .await;
}

async fn handle_response(
    requests: &mut JoinSet<CompletionResponse>,
    is_incomplete: bool,
) -> Option<CompletionResponse> {
    loop {
        let response = match requests.join_next().await? {
            Ok(response) => response,
            Err(error) => {
                log::error!("completion provider failed: {error}");
                continue;
            }
        };
        if !is_incomplete && !response.context.is_incomplete && response.items.is_empty() {
            continue;
        }
        return Some(response);
    }
}

async fn replace_completions(
    session: CompletionSession,
    mut requests: JoinSet<CompletionResponse>,
    is_incomplete: bool,
    request: Option<TaskHandle>,
) {
    while let Some(response) = handle_response(&mut requests, is_incomplete).await {
        deliver(
            session.clone(),
            request.clone(),
            Payload::Provider {
                response,
                is_incomplete,
            },
        )
        .await;
    }
}

// Invalidation can precede frontend delivery. Discard presentation state immediately.
pub(crate) fn invalidate(editor: &mut Editor) {
    let handler = &editor.handlers.completions;
    if handler.session.is_none() && handler.shared.trigger.lock().is_none() {
        return;
    }
    editor.handlers.completions.invalidate();
}

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        invalidate(event.editor);
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        let handler = &mut event.editor.handlers.completions;
        let active = handler
            .shared
            .trigger
            .lock()
            .is_some_and(|trigger| trigger.doc == event.doc.id());
        if active {
            invalidate(event.editor);
        }
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        let handler = &mut event.editor.handlers.completions;
        if handler
            .active_completions
            .contains_key(&CompletionProvider::Lsp(event.server_id))
        {
            invalidate(event.editor);
        }
        Ok(())
    });
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        let handler = &mut event.editor.handlers.completions;
        *handler.shared.timeout.lock() = event.new.completion.trigger_delay;
        let automatic = handler
            .shared
            .trigger
            .lock()
            .is_some_and(|trigger| trigger.kind != TriggerKind::Manual);
        if (event.old.lsp.enable && !event.new.lsp.enable)
            || (automatic && !event.new.completion.auto_trigger)
        {
            invalidate(event.editor);
        }
        Ok(())
    });
}
#[derive(Clone)]
pub struct ResponseContext {
    /// Whether the completion response is marked as "incomplete."
    ///
    /// This is used by LSP. When completions are "incomplete" and you continue typing, the
    /// completions should be recomputed by the server instead of filtered.
    pub is_incomplete: bool,
    pub priority: i8,
    pub savepoint: Arc<SavePoint>,
}

pub enum CompletionEvent {
    /// Auto completion was triggered by typing a word char
    AutoTrigger {
        cursor: usize,
        doc: DocumentId,
        view: ViewId,
    },
    /// Auto completion was triggered by typing a trigger char
    /// specified by the LSP
    TriggerChar {
        cursor: usize,
        doc: DocumentId,
        view: ViewId,
    },
    /// A completion was manually requested (c-x)
    ManualTrigger {
        cursor: usize,
        doc: DocumentId,
        view: ViewId,
    },
    /// Some text was deleted and the cursor is now at `pos`
    DeleteText { cursor: usize },
    /// Invalidate the current auto completion trigger
    Cancel,
}
pub fn trigger_auto_completion(editor: &Editor, trigger_char_only: bool) {
    let config = editor.config.load();
    if !config.completion.auto_trigger {
        return;
    }
    let (view, doc): (&crate::View, &crate::Document) = current_ref!(editor);
    let mut text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    text = doc.text().slice_chars(..cursor);

    let is_trigger_char = doc
        .language_servers_with_feature(LanguageServerFeature::Completion)
        .any(|ls| {
            matches!(&ls.capabilities().completion_provider, Some(lsp::CompletionOptions {
                        trigger_characters: Some(triggers),
                        ..
                    }) if triggers.iter().any(|trigger| text.ends_with(trigger)))
        });

    let cursor_char = text.bytes_at(text.len()).reversed().next();

    #[cfg(windows)]
    let is_path_completion_trigger = matches!(cursor_char, Some(b'/' | b'\\'));
    #[cfg(not(windows))]
    let is_path_completion_trigger = matches!(cursor_char, Some(b'/'));

    let handler = &editor.handlers.completions;
    if is_trigger_char || (is_path_completion_trigger && doc.path_completion_enabled()) {
        handler.event(CompletionEvent::TriggerChar {
            cursor,
            doc: doc.id(),
            view: view.id,
        });
        return;
    }

    let is_auto_trigger = !trigger_char_only
        && doc
            .text()
            .chars_at_char(cursor)
            .reversed()
            .take(config.completion.trigger_length as usize)
            .all(char_is_word);

    if is_auto_trigger {
        handler.event(CompletionEvent::AutoTrigger {
            cursor,
            doc: doc.id(),
            view: view.id,
        });
    }
}

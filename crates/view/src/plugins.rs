//! Per-editor WASM plugins and their bridge to typed editor events.
//!
//! The lifecycle, command discovery and event integration follow Helix PR #8675.
//! Guest calls use owned data and run outside event dispatch; no editor references
//! or syntax handles cross the WASM boundary.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
};

use ::plugins::{PluginCommand, PluginConfig, PluginManager};
use anyhow::{bail, ensure, Context};
use editor_core::{Range, Rope, Selection, Transaction};
use parking_lot::Mutex;
use plugin_sdk::{
    Action, DocumentSnapshot, EditorContext, Event, Response, SelectionRange, ViewSnapshot,
};
use serde_json::Value;

use crate::{callbacks::EditorCallbackSender, Document, DocumentId, Editor, ViewId};

const MAX_SNAPSHOT_BYTES: usize = ::plugins::MAX_MESSAGE_BYTES / 2;
const MAX_PENDING_EVENTS: usize = 32;

struct PendingEvent {
    event: Event,
    editor: EditorContext,
    data: Value,
}

#[derive(Default)]
struct Shared {
    pending: Mutex<VecDeque<PendingEvent>>,
    subscriptions: HashSet<Event>,
    applying: AtomicBool,
    generation: u64,
}

#[derive(Default)]
pub(crate) struct PluginHost {
    manager: PluginManager,
    shared: Arc<Shared>,
    stopped: bool,
}

impl PluginHost {
    pub(crate) fn sender(&self, callbacks: &EditorCallbackSender) -> Option<PluginEventSender> {
        (!self.shared.subscriptions.is_empty()).then(|| PluginEventSender {
            owner: Arc::downgrade(&self.shared),
            callbacks: callbacks.clone(),
        })
    }
}

/// A weak, editor-specific destination. Replacing a plugin host invalidates all
/// callbacks and document senders from the previous generation.
#[derive(Clone)]
pub(crate) struct PluginEventSender {
    owner: Weak<Shared>,
    callbacks: EditorCallbackSender,
}

impl PluginEventSender {
    fn interested(&self, event: Event) -> bool {
        self.owner.upgrade().is_some_and(|owner| {
            owner.subscriptions.contains(&event) && !owner.applying.load(Ordering::Relaxed)
        })
    }

    fn enqueue(&self, event: Event, editor: EditorContext, data: Value) {
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        if !self.interested(event) {
            return;
        }
        let mut pending = owner.pending.lock();
        if matches!(event, Event::DocumentChanged | Event::SelectionChanged) {
            let document = editor.document.as_ref().map(|doc| doc.id);
            let view = editor.view.as_ref().map(|view| view.id);
            pending.retain(|item| {
                item.event != event
                    || item.editor.document.as_ref().map(|doc| doc.id) != document
                    || (event == Event::SelectionChanged
                        && item.editor.view.as_ref().map(|view| view.id) != view)
            });
        }
        if pending.len() == MAX_PENDING_EVENTS {
            pending.pop_front();
        }
        pending.push_back(PendingEvent {
            event,
            editor,
            data,
        });
        drop(pending);
        let owner = self.owner.clone();
        self.callbacks.send_blocking(move |editor| {
            if let Some(owner) = owner.upgrade()
                && Arc::ptr_eq(&owner, &editor.plugins.shared)
            {
                editor.drain_plugin_events();
            }
        });
    }

    fn document(&self, event: Event, doc: &Document, view: Option<ViewId>) {
        if !self.interested(event) {
            return;
        }
        if let Ok(document) = snapshot(doc) {
            self.enqueue(
                event,
                EditorContext {
                    generation: self.owner.upgrade().map_or(0, |owner| owner.generation),
                    mode: String::new(),
                    document: Some(document),
                    view: view.and_then(|view| view_snapshot(doc, view)),
                },
                Value::Null,
            );
        }
    }
}

fn snapshot(doc: &Document) -> anyhow::Result<DocumentSnapshot> {
    ensure!(
        doc.text().len_bytes() <= MAX_SNAPSHOT_BYTES,
        "document exceeds the plugin snapshot limit ({MAX_SNAPSHOT_BYTES} bytes)"
    );
    Ok(DocumentSnapshot {
        id: doc.id().as_u64(),
        version: doc.version(),
        path: doc.path().map(|path| path.to_string_lossy().into_owned()),
        language: doc.language_name().map(str::to_owned),
        text: doc.text().to_string(),
    })
}

fn view_snapshot(doc: &Document, view: ViewId) -> Option<ViewSnapshot> {
    let selection = doc.selections().get(&view)?;
    Some(ViewSnapshot {
        id: view.as_u64(),
        document: doc.id().as_u64(),
        binding_revision: doc.view_binding_revision(view)?,
        selection_revision: doc.selection_revision(view)?,
        selections: selection
            .iter()
            .map(|range| SelectionRange {
                anchor: range.anchor,
                head: range.head,
            })
            .collect(),
        primary: selection.primary_index(),
    })
}

/// A rejected effect is distinguishable from an invalid action or a guest trap.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PluginConflict {
    #[error("plugin generation has changed")]
    GenerationChanged,
    #[error("document {0} is closed")]
    DocumentClosed(u64),
    #[error("stale document version for {0}")]
    DocumentChanged(u64),
    #[error("view {0} is closed")]
    ViewClosed(u64),
    #[error("view {0} has been rebound")]
    ViewRebound(u64),
    #[error("stale selection revision for view {0}")]
    SelectionChanged(u64),
}

impl Editor {
    fn plugin_context_for_view(&self, origin: Option<ViewId>) -> anyhow::Result<EditorContext> {
        let view = origin.and_then(|id| self.tree.try_get(id));
        let doc = view.and_then(|view| self.document(view.doc));
        Ok(EditorContext {
            generation: self.plugins.shared.generation,
            mode: self.mode.to_string(),
            document: doc.map(snapshot).transpose()?,
            view: view
                .zip(doc)
                .and_then(|(view, doc)| view_snapshot(doc, view.id)),
        })
    }

    fn plugin_context(&self) -> anyhow::Result<EditorContext> {
        self.plugin_context_for_view(Some(self.tree.focus))
    }

    pub fn plugin_commands(&self) -> Vec<PluginCommand> {
        self.plugins.manager.available_commands()
    }

    pub fn plugin_command_doc(&self, name: &str) -> Option<String> {
        self.plugins.manager.get_doc_for_identifier(name)
    }

    /// Replace plugin instances, discard stale callbacks and initialize the new
    /// generation. A broken plugin cannot prevent other plugins from loading.
    pub fn reload_plugins(&mut self, config: &BTreeMap<String, PluginConfig>, base: &Path) -> bool {
        let (manager, errors) = PluginManager::load(config, base);
        let generation = self
            .plugins
            .shared
            .generation
            .checked_add(1)
            .expect("plugin generation exhausted");
        self.shutdown_plugins();
        let subscriptions = [
            Event::DocumentOpened,
            Event::DocumentChanged,
            Event::DocumentSaved,
            Event::DocumentClosed,
            Event::SelectionChanged,
            Event::ModeChanged,
            Event::PostCommand,
            Event::PostInsertChar,
            Event::DocumentFocusLost,
            Event::TerminalFocusGained,
            Event::TerminalFocusLost,
        ]
        .into_iter()
        .filter(|event| manager.subscribes(*event))
        .collect();
        self.plugins = PluginHost {
            manager,
            stopped: false,
            shared: Arc::new(Shared {
                subscriptions,
                generation,
                ..Shared::default()
            }),
        };
        let sender = self.plugins.sender(&self.handlers.callbacks);
        for doc in self.documents.values_mut() {
            doc.plugin_events = sender.clone();
        }
        let initialized = self.dispatch_plugin_event(Event::Init, Value::Null);
        let loaded = errors.is_empty();
        for error in errors {
            log::error!("{error}");
            self.set_error(|| error);
        }
        loaded && initialized
    }

    pub fn shutdown_plugins(&mut self) {
        if self.plugins.stopped {
            return;
        }
        self.plugins.stopped = true;
        self.dispatch_plugin_event(Event::Shutdown, Value::Null);
        self.plugins.shared = Arc::new(Shared {
            generation: self.plugins.shared.generation,
            ..Shared::default()
        });
    }

    pub fn execute_plugin_command(
        &mut self,
        name: &str,
        args: Vec<String>,
    ) -> anyhow::Result<bool> {
        ensure!(!self.plugins.stopped, "plugins have been shut down");
        if self.plugin_command_doc(name).is_none() {
            return Ok(false);
        }
        let context = self.plugin_context()?;
        let Some(response) = self
            .plugins
            .manager
            .call_command(name, args, context.clone())?
        else {
            return Ok(false);
        };
        self.apply_plugin_response(response, &context)
            .with_context(|| format!("plugin command '{name}'"))?;
        Ok(true)
    }

    pub fn dispatch_plugin_event(&mut self, event: Event, data: Value) -> bool {
        let context = match self.plugin_context() {
            Ok(context) => context,
            Err(_) if matches!(event, Event::Init | Event::Shutdown) => EditorContext {
                generation: self.plugins.shared.generation,
                mode: self.mode.to_string(),
                document: None,
                view: None,
            },
            Err(err) => {
                log::warn!("plugin event skipped: {err}");
                return false;
            }
        };
        self.run_plugin_event(event, context, data)
    }

    /// Queue frontend events on the same owning-editor path as document hooks.
    pub fn queue_plugin_event(&self, event: Event, data: Value) {
        self.queue_plugin_event_for_view(event, self.tree.focus, data);
    }

    /// Captures the originating split rather than whichever split later gains focus.
    /// A closed origin still produces the event, with no document or view snapshot.
    pub fn queue_plugin_event_for_view(&self, event: Event, view: ViewId, data: Value) {
        let Some(sender) = self.plugins.sender(&self.handlers.callbacks) else {
            return;
        };
        if sender.interested(event)
            && let Ok(context) = self.plugin_context_for_view(Some(view))
        {
            sender.enqueue(event, context, data);
        }
    }

    pub fn queue_plugin_document_saved(&self, id: DocumentId) {
        self.queue_plugin_document_event(Event::DocumentSaved, id);
    }

    pub(crate) fn queue_plugin_document_event(&self, event: Event, id: DocumentId) {
        if let Some(sender) = self.plugins.sender(&self.handlers.callbacks)
            && let Some(doc) = self.document(id)
        {
            sender.document(event, doc, None);
        }
    }

    fn drain_plugin_events(&mut self) {
        let pending = self
            .plugins
            .shared
            .pending
            .lock()
            .drain(..)
            .collect::<Vec<_>>();
        for mut pending in pending {
            if pending.editor.mode.is_empty() {
                pending.editor.mode = self.mode.to_string();
            }
            self.run_plugin_event(pending.event, pending.editor, pending.data);
        }
    }

    fn run_plugin_event(&mut self, event: Event, context: EditorContext, data: Value) -> bool {
        let mut successful = true;
        for (name, result) in self
            .plugins
            .manager
            .dispatch_event(event, context.clone(), data)
        {
            if let Err(err) =
                result.and_then(|response| self.apply_plugin_response(response, &context))
            {
                successful = false;
                log::error!("plugin '{name}': {err:#}");
                self.set_error(|| format!("plugin '{name}': {err:#}"));
            }
        }
        successful
    }

    fn apply_plugin_response(
        &mut self,
        response: Response,
        context: &EditorContext,
    ) -> anyhow::Result<()> {
        if context.generation != self.plugins.shared.generation {
            return Err(PluginConflict::GenerationChanged.into());
        }
        if let Some(error) = response.error {
            bail!("{error}")
        }
        // Every document/view precondition and every projected range is checked
        // before any effect is applied. Later edits use the preceding edit's text.
        let mut documents = BTreeMap::<DocumentId, PreparedDocument>::new();
        let mut prepared = Vec::new();
        let mut opened = false;
        for action in response.actions {
            match action {
                Action::Edit {
                    document,
                    version,
                    mut edits,
                } => {
                    ensure!(!opened, "edit actions must precede open actions");
                    let id = self.plugin_document(document, version)?;
                    let doc = &self.documents[&id];
                    ensure!(!doc.readonly, "cannot edit readonly document {document}");
                    ensure!(!doc.is_binary(), "cannot edit binary document {document}");
                    let plan = documents
                        .entry(id)
                        .or_insert_with(|| PreparedDocument::new(doc.text()));
                    edits.sort_by_key(|edit| (edit.start, edit.end));
                    let mut end = 0;
                    for edit in &edits {
                        ensure!(
                            edit.start >= end
                                && edit.start <= edit.end
                                && edit.end <= plan.text.len_chars(),
                            "invalid or overlapping edit range for document {document}"
                        );
                        end = edit.end;
                    }
                    let transaction = Transaction::change(
                        &plan.text,
                        edits
                            .into_iter()
                            .map(|edit| (edit.start, edit.end, Some(edit.text.into()))),
                    );
                    ensure!(
                        transaction.apply(&mut plan.text),
                        "invalid plugin transaction"
                    );
                    for selection in plan.selections.values_mut() {
                        *selection = selection
                            .clone()
                            .map(transaction.changes())
                            .ensure_invariants(plan.text.slice(..));
                    }
                    plan.transaction = Some(match plan.transaction.take() {
                        Some(previous) => previous.compose(transaction),
                        None => transaction,
                    });
                }
                Action::SetSelection {
                    document,
                    version,
                    view,
                    binding_revision,
                    selection_revision,
                    ranges,
                    primary,
                } => {
                    ensure!(!opened, "selection actions must precede open actions");
                    let id = self.plugin_document(document, version)?;
                    let view_id = ViewId::from_u64(view);
                    let target = self
                        .tree
                        .try_get(view_id)
                        .ok_or(PluginConflict::ViewClosed(view))?;
                    if target.doc != id || target.binding_revision() != binding_revision {
                        return Err(PluginConflict::ViewRebound(view).into());
                    }
                    let doc = &self.documents[&id];
                    if doc.selection_revision(view_id) != Some(selection_revision) {
                        return Err(PluginConflict::SelectionChanged(view).into());
                    }
                    let plan = documents
                        .entry(id)
                        .or_insert_with(|| PreparedDocument::new(doc.text()));
                    ensure!(
                        !ranges.is_empty() && primary < ranges.len(),
                        "invalid primary selection"
                    );
                    ensure!(
                        ranges
                            .iter()
                            .all(|range| range.anchor <= plan.text.len_chars()
                                && range.head <= plan.text.len_chars()),
                        "selection outside document {document}"
                    );
                    plan.selections.insert(
                        view_id,
                        Selection::new(
                            ranges
                                .into_iter()
                                .map(|range| Range::new(range.anchor, range.head))
                                .collect(),
                            primary,
                        )
                        .ensure_invariants(plan.text.slice(..)),
                    );
                }
                Action::Status { message } => prepared.push(PreparedAction::Status(message)),
                Action::Error { message } => prepared.push(PreparedAction::Error(message)),
                Action::Open { path } => {
                    opened = true;
                    ensure!(!path.is_empty(), "plugin open path is empty");
                    prepared.push(PreparedAction::Open(path));
                }
            }
        }
        let owner = self.plugins.shared.clone();
        owner.applying.store(true, Ordering::Relaxed);
        let _guard = ApplyingGuard(owner);
        for (id, mut plan) in documents {
            let doc = self.documents.get_mut(&id).unwrap();
            if plan.transaction.is_some() {
                // Commit native changes first. Synchronize from history, so a stale
                // background split does not skip older edits when the plugin commits.
                doc.commit_pending_changes();
                for (view, _) in self.tree.views_mut() {
                    view.sync_changes(doc);
                }
            }
            if let Some(mut transaction) = plan.transaction {
                let origin = context
                    .view
                    .as_ref()
                    .filter(|origin| origin.document == id.as_u64())
                    .map(|origin| ViewId::from_u64(origin.id))
                    .filter(|view| {
                        self.tree.try_get(*view).is_some_and(|target| {
                            target.doc == id
                                && context.view.as_ref().is_some_and(|origin| {
                                    target.binding_revision() == origin.binding_revision
                                })
                        })
                    });
                let history_view = origin.or_else(|| {
                    plan.selections
                        .keys()
                        .copied()
                        .min_by_key(|view| view.as_u64())
                });
                if let Some(view) = history_view {
                    let selection = plan.selections.remove(&view).unwrap_or_else(|| {
                        doc.selection(view)
                            .clone()
                            .map(transaction.changes())
                            .ensure_invariants(plan.text.slice(..))
                    });
                    transaction = transaction.with_selection(selection);
                }
                ensure!(
                    doc.apply_plugin_transaction(&transaction, history_view),
                    "plugin edit failed"
                );
                for (view, _) in self.tree.views_mut() {
                    view.sync_changes(doc);
                }
            }
            for (view, selection) in plan.selections {
                doc.set_selection(view, selection);
            }
        }
        for action in prepared {
            match action {
                PreparedAction::Status(message) => self.set_status(message),
                PreparedAction::Error(message) => self.set_error(|| message),
                PreparedAction::Open(path) => {
                    let action = if self.tree.views().next().is_none() {
                        crate::editor::Action::VerticalSplit
                    } else {
                        crate::editor::Action::Replace
                    };
                    self.open(Path::new(&path), action)?;
                }
            }
        }
        Ok(())
    }

    fn plugin_document(&self, number: u64, version: i32) -> anyhow::Result<DocumentId> {
        let doc = self
            .documents
            .values()
            .find(|doc| doc.id().as_u64() == number)
            .ok_or(PluginConflict::DocumentClosed(number))?;
        if doc.version() != version {
            return Err(PluginConflict::DocumentChanged(number).into());
        }
        Ok(doc.id())
    }
}

struct ApplyingGuard(Arc<Shared>);
impl Drop for ApplyingGuard {
    fn drop(&mut self) {
        self.0.applying.store(false, Ordering::Relaxed);
    }
}

struct PreparedDocument {
    text: Rope,
    transaction: Option<Transaction>,
    selections: std::collections::HashMap<ViewId, Selection>,
}

impl PreparedDocument {
    fn new(text: &Rope) -> Self {
        Self {
            text: text.clone(),
            transaction: None,
            selections: Default::default(),
        }
    }
}

enum PreparedAction {
    Status(String),
    Error(String),
    Open(String),
}

pub(crate) fn register_hooks() {
    use crate::events::{DocumentDidChange, DocumentDidClose, DocumentDidOpen, SelectionDidChange};
    event::register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        if let Some(sender) = event
            .editor
            .plugins
            .sender(&event.editor.handlers.callbacks)
            && let Some(doc) = event.editor.document(event.doc)
        {
            sender.document(Event::DocumentOpened, doc, None);
        }
        Ok(())
    });
    event::register_hook!(move |event: &mut DocumentDidClose<'_>| {
        if let Some(sender) = event
            .editor
            .plugins
            .sender(&event.editor.handlers.callbacks)
        {
            sender.document(Event::DocumentClosed, &event.doc, None);
        }
        Ok(())
    });
    event::register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if !event.ghost_transaction
            && let Some(sender) = &event.doc.plugin_events
        {
            sender.document(Event::DocumentChanged, event.doc, event.view);
        }
        Ok(())
    });
    event::register_hook!(move |event: &mut SelectionDidChange<'_>| {
        if let Some(sender) = &event.doc.plugin_events {
            sender.document(Event::SelectionChanged, event.doc, Some(event.view));
        }
        Ok(())
    });
}

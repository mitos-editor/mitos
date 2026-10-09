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
use anyhow::{anyhow, bail, ensure, Context};
use editor_core::{Range, Rope, Selection, Transaction};
use parking_lot::Mutex;
use plugin_sdk::{Action, DocumentSnapshot, EditorContext, Event, Response, SelectionRange};
use serde_json::Value;

use crate::{
    callbacks::EditorCallbackSender, document::Mode, Document, DocumentId, Editor, ViewId,
};

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
            pending.retain(|item| {
                item.event != event || item.editor.document.as_ref().map(|doc| doc.id) != document
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
        if let Ok(document) = snapshot(doc, view) {
            self.enqueue(
                event,
                EditorContext {
                    mode: String::new(),
                    document: Some(document),
                },
                Value::Null,
            );
        }
    }
}

fn snapshot(doc: &Document, view: Option<ViewId>) -> anyhow::Result<DocumentSnapshot> {
    ensure!(
        doc.text().len_bytes() <= MAX_SNAPSHOT_BYTES,
        "document exceeds the plugin snapshot limit ({MAX_SNAPSHOT_BYTES} bytes)"
    );
    let selection = view
        .and_then(|id| doc.selections().get(&id))
        .or_else(|| doc.selections().values().next());
    let selections = selection.map_or_else(
        || vec![SelectionRange { anchor: 0, head: 0 }],
        |selection| {
            selection
                .iter()
                .map(|range| SelectionRange {
                    anchor: range.anchor,
                    head: range.head,
                })
                .collect()
        },
    );
    Ok(DocumentSnapshot {
        id: doc.id().as_u64(),
        version: doc.version(),
        path: doc.path().map(|path| path.to_string_lossy().into_owned()),
        language: doc.language_name().map(str::to_owned),
        text: doc.text().to_string(),
        selections,
        primary: selection.map_or(0, Selection::primary_index),
    })
}

impl Editor {
    fn plugin_context(&self) -> anyhow::Result<EditorContext> {
        let document = self
            .tree
            .views()
            .find(|(_, focused)| *focused)
            .and_then(|(view, _)| self.document(view.doc).map(|doc| (view, doc)))
            .map(|(view, doc)| snapshot(doc, Some(view.id)))
            .transpose()?;
        Ok(EditorContext {
            mode: match self.mode {
                Mode::Normal => "normal",
                Mode::Select => "select",
                Mode::Insert => "insert",
            }
            .into(),
            document,
        })
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
        self.shutdown_plugins();
        let subscriptions = [
            Event::DocumentOpened,
            Event::DocumentChanged,
            Event::DocumentSaved,
            Event::DocumentClosed,
            Event::SelectionChanged,
            Event::ModeChanged,
            Event::PostCommand,
        ]
        .into_iter()
        .filter(|event| manager.subscribes(*event))
        .collect();
        self.plugins = PluginHost {
            manager,
            stopped: false,
            shared: Arc::new(Shared {
                subscriptions,
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
        self.plugins.shared = Arc::default();
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
        let Some(response) = self.plugins.manager.call_command(name, args, context)? else {
            return Ok(false);
        };
        self.apply_plugin_response(response)
            .with_context(|| format!("plugin command '{name}'"))?;
        Ok(true)
    }

    pub fn dispatch_plugin_event(&mut self, event: Event, data: Value) -> bool {
        let context = match self.plugin_context() {
            Ok(context) => context,
            Err(_) if matches!(event, Event::Init | Event::Shutdown) => EditorContext {
                mode: self.mode.to_string(),
                document: None,
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
        let Some(sender) = self.plugins.sender(&self.handlers.callbacks) else {
            return;
        };
        if sender.interested(event)
            && let Ok(context) = self.plugin_context()
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
            let view = self
                .tree
                .views()
                .find(|(view, _)| view.doc == id)
                .map(|(view, _)| view.id);
            sender.document(event, doc, view);
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
                pending.editor.mode = match self.mode {
                    Mode::Normal => "normal",
                    Mode::Select => "select",
                    Mode::Insert => "insert",
                }
                .into();
            }
            self.run_plugin_event(pending.event, pending.editor, pending.data);
        }
    }

    fn run_plugin_event(&mut self, event: Event, context: EditorContext, data: Value) -> bool {
        let mut successful = true;
        for (name, result) in self.plugins.manager.dispatch_event(event, context, data) {
            if let Err(err) = result.and_then(|response| self.apply_plugin_response(response)) {
                successful = false;
                log::error!("plugin '{name}': {err:#}");
                self.set_error(|| format!("plugin '{name}': {err:#}"));
            }
        }
        successful
    }

    fn apply_plugin_response(&mut self, response: Response) -> anyhow::Result<()> {
        if let Some(error) = response.error {
            bail!("{error}")
        }
        // Validate the entire batch before modifying any documents. Project the
        // text lengths so selections following edits are checked against new text.
        let mut texts = BTreeMap::<DocumentId, Rope>::new();
        let mut edited = HashSet::new();
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
                    let (id, view) = self.plugin_document_view(document)?;
                    let doc = &self.documents[&id];
                    ensure!(!doc.is_binary(), "cannot edit binary document {document}");
                    ensure!(
                        doc.version() == version,
                        "stale document version for {document}"
                    );
                    ensure!(
                        edited.insert(id),
                        "only one edit action per document is allowed in a response"
                    );
                    edits.sort_by_key(|edit| (edit.start, edit.end));
                    let mut end = 0;
                    for edit in &edits {
                        ensure!(
                            edit.start >= end
                                && edit.start <= edit.end
                                && edit.end <= doc.text().len_chars(),
                            "invalid or overlapping edit range for document {document}"
                        );
                        end = edit.end;
                    }
                    let transaction = Transaction::change(
                        doc.text(),
                        edits
                            .into_iter()
                            .map(|edit| (edit.start, edit.end, Some(edit.text.into()))),
                    );
                    let mut text = doc.text().clone();
                    ensure!(transaction.apply(&mut text), "invalid plugin transaction");
                    texts.insert(id, text);
                    prepared.push(PreparedAction::Edit {
                        id,
                        view,
                        transaction,
                    });
                }
                Action::SetSelection {
                    document,
                    version,
                    ranges,
                    primary,
                } => {
                    ensure!(!opened, "selection actions must precede open actions");
                    let (id, view) = self.plugin_document_view(document)?;
                    ensure!(
                        self.documents[&id].version() == version,
                        "stale document version for {document}"
                    );
                    let text = texts.get(&id).unwrap_or_else(|| self.documents[&id].text());
                    ensure!(
                        !ranges.is_empty() && primary < ranges.len(),
                        "invalid primary selection"
                    );
                    ensure!(
                        ranges.iter().all(|range| range.anchor <= text.len_chars()
                            && range.head <= text.len_chars()),
                        "selection outside document {document}"
                    );
                    let selection = Selection::new(
                        ranges
                            .into_iter()
                            .map(|range| Range::new(range.anchor, range.head))
                            .collect(),
                        primary,
                    );
                    if let Some(PreparedAction::Edit { transaction, .. }) = prepared.iter_mut().rev()
                        .find(|action| matches!(action, PreparedAction::Edit { id: target, .. } if *target == id))
                    {
                        // Keep the guest's final selection in the edit's undo/redo
                        // transaction, rather than committing its mapped old selection.
                        *transaction = transaction.clone().with_selection(selection);
                    } else {
                        prepared.push(PreparedAction::Selection { id, view, selection });
                    }
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
        for action in prepared {
            match action {
                PreparedAction::Edit {
                    id,
                    view,
                    transaction,
                } => {
                    let doc = self.documents.get_mut(&id).unwrap();
                    doc.append_changes_to_history(self.tree.get_mut(view));
                    ensure!(doc.apply(&transaction, view), "plugin edit failed");
                    doc.append_changes_to_history(self.tree.get_mut(view));
                }
                PreparedAction::Selection {
                    id,
                    view,
                    selection,
                } => {
                    self.documents
                        .get_mut(&id)
                        .unwrap()
                        .set_selection(view, selection);
                }
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

    fn plugin_document_view(&self, number: u64) -> anyhow::Result<(DocumentId, ViewId)> {
        let doc = self
            .documents
            .values()
            .find(|doc| doc.id().as_u64() == number)
            .ok_or_else(|| anyhow!("no document {number}"))?;
        let view = self
            .tree
            .views()
            .filter(|(view, _)| view.doc == doc.id())
            .max_by_key(|(_, focused)| *focused)
            .map(|(view, _)| view.id)
            .ok_or_else(|| anyhow!("document {number} has no visible view"))?;
        Ok((doc.id(), view))
    }
}

struct ApplyingGuard(Arc<Shared>);
impl Drop for ApplyingGuard {
    fn drop(&mut self) {
        self.0.applying.store(false, Ordering::Relaxed);
    }
}

enum PreparedAction {
    Edit {
        id: DocumentId,
        view: ViewId,
        transaction: Transaction,
    },
    Selection {
        id: DocumentId,
        view: ViewId,
        selection: Selection,
    },
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
            sender.document(Event::DocumentChanged, event.doc, Some(event.view));
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

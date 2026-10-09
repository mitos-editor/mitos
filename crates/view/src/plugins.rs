//! Per-editor WASM plugins and their bridge to typed editor events.
//!
//! The lifecycle, command discovery and event integration follow Helix PR #8675.
//! Guest calls use owned data and run outside event dispatch; no editor references
//! or syntax handles cross the WASM boundary.

mod actor;

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    path::Path,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use ::plugins::{InvocationTarget, PluginCommand, PluginConfig, PluginManager};
use anyhow::{bail, ensure};
use editor_core::{Range, Rope, Selection, Transaction};
use parking_lot::Mutex;
use plugin_api::{
    Action, Capability, DocumentInfo, DocumentSnapshot, EditorContext, ErrorCode, Event, Response,
    SelectionRange, ServiceError, StateCatalog, StateQuery, ViewInfo, ViewSnapshot,
};
use serde_json::Value;

use crate::{Document, DocumentId, Editor, ViewId, callbacks::EditorCallbackSender};

const MAX_PENDING_EVENTS: usize = 32;
const MAX_CONTROL_EVENTS: usize = 32;
const MAX_PENDING_BYTES: usize = ::plugins::MAX_MESSAGE_BYTES * 2;
const MAX_CAUSAL_DEPTH: u16 = 8;
const MAX_DRAIN_EVENTS: usize = 64;
const MAX_STATUS_BYTES: usize = 4096;

const OBSERVABLE_EVENTS: &[Event] = &[
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
    Event::State,
    Event::ResyncRequired,
    Event::UiResult,
    Event::BuiltinResult,
    Event::KeymapResult,
    Event::JobReady,
];

#[derive(Clone, Debug)]
struct EffectOrigin {
    plugin: String,
    sequence: u64,
    depth: u16,
}

/// Owned frontend provenance; its fields are assigned only by this editor.
/// A delayed completion cannot re-enter an unloaded plugin generation.
#[derive(Clone, Debug)]
pub struct PluginEventSource {
    generation: u64,
    origin: Option<EffectOrigin>,
}

#[derive(Clone, serde::Serialize)]
struct Provenance {
    generation: u64,
    sequence: u64,
    parent_sequence: Option<u64>,
    origin_plugin: Option<String>,
    depth: u16,
}

struct PendingEvent {
    event: Event,
    editor: EditorContext,
    data: Value,
    provenance: Provenance,
    target: Option<String>,
    scope: InvocationTarget,
    bytes: usize,
}

fn frontend_reply(event: Event) -> bool {
    matches!(
        event,
        Event::UiResult | Event::BuiltinResult | Event::KeymapResult
    )
}

fn control(event: Event) -> bool {
    if frontend_reply(event) || event == Event::JobReady {
        return true;
    }
    matches!(
        event,
        Event::DocumentOpened
            | Event::DocumentSaved
            | Event::DocumentClosed
            | Event::DocumentFocusLost
            | Event::State
            | Event::ResyncRequired
    )
}

fn queue_class(event: Event) -> u8 {
    if frontend_reply(event) {
        2
    } else if event == Event::JobReady {
        3
    } else if control(event) {
        1
    } else {
        0
    }
}

#[derive(Default, serde::Serialize)]
struct Gap {
    first_sequence: u64,
    last_sequence: u64,
    dropped: usize,
    reasons: std::collections::BTreeSet<&'static str>,
}

#[derive(Default)]
struct Queue {
    pending: VecDeque<PendingEvent>,
    bytes: usize,
    gap: Option<Gap>,
    sequence: u64,
    wake_scheduled: bool,
    origin: Option<EffectOrigin>,
}

impl Queue {
    fn provenance(&mut self, generation: u64) -> Provenance {
        self.sequence = self
            .sequence
            .checked_add(1)
            .expect("plugin event sequence exhausted");
        Provenance {
            generation,
            sequence: self.sequence,
            parent_sequence: self.origin.as_ref().map(|origin| origin.sequence),
            origin_plugin: self.origin.as_ref().map(|origin| origin.plugin.clone()),
            depth: self
                .origin
                .as_ref()
                .map_or(0, |origin| origin.depth.saturating_add(1)),
        }
    }

    fn lost(&mut self, sequence: u64, reason: &'static str) {
        let gap = self.gap.get_or_insert_with(|| Gap {
            first_sequence: sequence,
            ..Gap::default()
        });
        gap.first_sequence = gap.first_sequence.min(sequence);
        gap.last_sequence = gap.last_sequence.max(sequence);
        gap.dropped = gap.dropped.saturating_add(1);
        gap.reasons.insert(reason);
    }

    fn remove(&mut self, index: usize) -> PendingEvent {
        let event = self.pending.remove(index).unwrap();
        self.bytes -= event.bytes;
        event
    }
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    subscriptions: Mutex<BTreeMap<Event, HashSet<String>>>,
    readers: Mutex<HashSet<String>>,
    accepting: AtomicBool,
    generation: u64,
    async_woken: AtomicBool,
    async_pending: AtomicBool,
}

#[derive(Default)]
pub(crate) struct PluginHost {
    manager: PluginManager,
    shared: Arc<Shared>,
    stopped: bool,
    shutting_down: bool,
    asynchronous: actor::AsyncState,
}

impl PluginHost {
    pub(crate) fn sender(&self, callbacks: &EditorCallbackSender) -> Option<PluginEventSender> {
        (!self.shared.subscriptions.lock().is_empty()
            && self.shared.accepting.load(Ordering::Relaxed))
        .then(|| PluginEventSender {
            owner: Arc::downgrade(&self.shared),
            callbacks: callbacks.clone(),
        })
    }
}

/// Weak ownership invalidates scheduled wakes and document senders on reload.
#[derive(Clone)]
pub(crate) struct PluginEventSender {
    owner: Weak<Shared>,
    callbacks: EditorCallbackSender,
}

impl PluginEventSender {
    fn interested(&self, event: Event) -> bool {
        let Some(owner) = self.owner.upgrade() else {
            return false;
        };
        if !owner.accepting.load(Ordering::Relaxed) {
            return false;
        }
        let origin = owner
            .queue
            .lock()
            .origin
            .as_ref()
            .map(|origin| origin.plugin.clone());
        owner
            .subscriptions
            .lock()
            .get(&event)
            .is_some_and(|targets| targets.iter().any(|target| origin.as_ref() != Some(target)))
    }

    fn needs_context(&self, event: Event) -> bool {
        let Some(owner) = self.owner.upgrade() else {
            return false;
        };
        let origin = owner
            .queue
            .lock()
            .origin
            .as_ref()
            .map(|origin| origin.plugin.clone());
        let readers = owner.readers.lock();
        owner
            .subscriptions
            .lock()
            .get(&event)
            .is_some_and(|targets| {
                targets
                    .iter()
                    .any(|target| origin.as_ref() != Some(target) && readers.contains(target))
            })
    }

    fn enqueue(&self, event: Event, editor: EditorContext, data: Value) {
        if self.interested(event) {
            self.enqueue_target(event, editor, data, None);
        }
    }

    fn enqueue_target(
        &self,
        event: Event,
        editor: EditorContext,
        data: Value,
        target: Option<String>,
    ) {
        let scope = InvocationTarget {
            document: editor.document.as_ref().map(|doc| doc.id),
            view: editor.view.as_ref().map(|view| view.id),
            binding_revision: editor.view.as_ref().map(|view| view.binding_revision),
        };
        self.enqueue_scoped(event, editor, data, target, scope);
    }

    fn enqueue_scoped(
        &self,
        event: Event,
        editor: EditorContext,
        data: Value,
        target: Option<String>,
        scope: InvocationTarget,
    ) {
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        if !owner.accepting.load(Ordering::Relaxed) {
            return;
        }
        let mut queue = owner.queue.lock();
        let provenance = queue.provenance(owner.generation);
        let bytes = json_size(&editor).saturating_add(json_size(&data));
        if (provenance.depth > MAX_CAUSAL_DEPTH
            && !frontend_reply(event)
            && event != Event::JobReady)
            || bytes > MAX_PENDING_BYTES
        {
            queue.lost(
                provenance.sequence,
                if bytes > MAX_PENDING_BYTES {
                    "snapshot-size"
                } else {
                    "causal-depth"
                },
            );
        } else {
            if matches!(
                event,
                Event::DocumentChanged | Event::SelectionChanged | Event::JobReady
            ) {
                let document = editor.document.as_ref().map(|doc| doc.id);
                let view = editor.view.as_ref().map(|view| view.id);
                if let Some(index) = queue.pending.iter().position(|item| {
                    item.event == event
                        && (event != Event::JobReady || item.data.get("job") == data.get("job"))
                        && item.editor.document.as_ref().map(|doc| doc.id) == document
                        && item.editor.view.as_ref().map(|view| view.id) == view
                        && item.target == target
                }) {
                    queue.remove(index);
                }
            }
            let limit = if frontend_reply(event) {
                8
            } else if event == Event::JobReady {
                32 // NativeBudget limits the owning editor to 32 live jobs.
            } else if control(event) {
                MAX_CONTROL_EVENTS
            } else {
                MAX_PENDING_EVENTS
            };
            while queue
                .pending
                .iter()
                .filter(|item| queue_class(item.event) == queue_class(event))
                .count()
                >= limit
                || queue.bytes.saturating_add(bytes) > MAX_PENDING_BYTES
            {
                let class_full = queue
                    .pending
                    .iter()
                    .filter(|item| queue_class(item.event) == queue_class(event))
                    .count()
                    >= limit;
                let index = if class_full {
                    queue
                        .pending
                        .iter()
                        .position(|item| queue_class(item.event) == queue_class(event))
                } else {
                    queue
                        .pending
                        .iter()
                        .position(|item| !control(item.event))
                        .or_else(|| {
                            queue.pending.iter().position(|item| {
                                !frontend_reply(item.event) && item.event != Event::JobReady
                            })
                        })
                };
                let Some(index) = index else {
                    break;
                };
                let removed = queue.remove(index);
                queue.lost(removed.provenance.sequence, "queue-capacity");
            }
            queue.bytes += bytes;
            queue.pending.push_back(PendingEvent {
                event,
                editor,
                data,
                provenance,
                target,
                scope,
                bytes,
            });
        }
        drop(queue);
        self.schedule_wake();
    }

    fn schedule_wake(&self) {
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        let mut queue = owner.queue.lock();
        if queue.wake_scheduled
            || (queue.pending.is_empty()
                && queue.gap.is_none()
                && !owner.async_woken.swap(false, Ordering::AcqRel))
        {
            return;
        }
        queue.wake_scheduled = true;
        drop(queue);
        let weak = self.owner.clone();
        if self
            .callbacks
            .try_send(move |editor| {
                if let Some(owner) = weak.upgrade()
                    && Arc::ptr_eq(&owner, &editor.plugins.shared)
                {
                    owner.queue.lock().wake_scheduled = false;
                    editor.poll_plugin_events();
                }
            })
            .is_err()
        {
            // Work remains in the bounded host queue. Frontends poll it on their
            // editor mutation path when callback capacity becomes available.
            owner.queue.lock().wake_scheduled = false;
        }
    }

    fn document(&self, event: Event, doc: &Document, view: Option<ViewId>) {
        if !self.interested(event) {
            return;
        }
        if view.is_some_and(|view| {
            doc.selections()
                .get(&view)
                .is_some_and(|selection| selection.len() > 1024)
        }) {
            if let Some(owner) = self.owner.upgrade() {
                let mut queue = owner.queue.lock();
                let sequence = queue.provenance(owner.generation).sequence;
                queue.lost(sequence, "selection-limit");
            }
            self.schedule_wake();
            return;
        }
        match snapshot(doc) {
            Ok(document) => self.enqueue(
                event,
                EditorContext {
                    generation: self.owner.upgrade().map_or(0, |owner| owner.generation),
                    mode: String::new(),
                    document: Some(document),
                    view: view.and_then(|view| view_snapshot(doc, view)),
                },
                Value::Null,
            ),
            Err(_) => {
                if let Some(owner) = self.owner.upgrade() {
                    let mut queue = owner.queue.lock();
                    let sequence = queue.provenance(owner.generation).sequence;
                    queue.lost(sequence, "snapshot-size");
                }
                self.schedule_wake();
            }
        }
    }
}

fn json_size(value: &impl serde::Serialize) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    let _ = serde_json::to_writer(&mut count, value);
    count.0
}

fn with_provenance(data: Value, provenance: &Provenance) -> Value {
    let mut fields = match data {
        Value::Object(fields) => fields,
        Value::Null => serde_json::Map::new(),
        payload => serde_json::Map::from_iter([("payload".into(), payload)]),
    };
    fields.insert(
        "provenance".into(),
        serde_json::to_value(provenance).unwrap(),
    );
    Value::Object(fields)
}

fn snapshot(doc: &Document) -> anyhow::Result<DocumentSnapshot> {
    Ok(DocumentSnapshot {
        id: doc.id().as_u64(),
        version: doc.version(),
        path: doc.path().map(|path| path.to_string_lossy().into_owned()),
        language: doc.language_name().map(str::to_owned),
        char_count: doc.text().len_chars() as u64,
        byte_count: doc.text().len_bytes() as u64,
    })
}

fn view_snapshot(doc: &Document, view: ViewId) -> Option<ViewSnapshot> {
    let selection = doc.selections().get(&view)?;
    if selection.len() > 1024 {
        return None;
    }
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

fn bounded_view_snapshot(doc: &Document, view: ViewId) -> anyhow::Result<Option<ViewSnapshot>> {
    if doc
        .selections()
        .get(&view)
        .is_some_and(|selection| selection.len() > 1024)
    {
        return Err(ServiceError::new(
            ErrorCode::ResourceExhausted,
            "view has more than 1024 selections; plugin snapshot omitted",
        )
        .into());
    }
    Ok(view_snapshot(doc, view))
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
    /// Page the current document/view catalogs, optionally retrieving one text
    /// and selection snapshot. Handles and cursors belong to this editor session.
    pub fn plugin_state(&self, query: &StateQuery) -> (EditorContext, StateCatalog) {
        let limit = if query.limit == 0 {
            64
        } else {
            query.limit.min(64)
        };
        let mut documents = self.documents.values().filter(|doc| {
            query
                .after_document
                .is_none_or(|after| doc.id().as_u64() > after)
        });
        let page = documents.by_ref().take(limit).collect::<Vec<_>>();
        let next_document = documents
            .next()
            .and_then(|_| page.last().map(|doc| doc.id().as_u64()));
        let documents = page
            .into_iter()
            .map(|doc| DocumentInfo {
                id: doc.id().as_u64(),
                version: doc.version(),
                path: doc.path().map(|path| path.to_string_lossy().into_owned()),
                language: doc.language_name().map(str::to_owned),
                readonly: doc.readonly,
                binary: doc.is_binary(),
                bytes: doc.text().len_bytes(),
                chars: doc.text().len_chars(),
            })
            .collect();
        let mut views = self
            .tree
            .views()
            .map(|(view, _)| view)
            .filter(|view| {
                query
                    .after_view
                    .is_none_or(|after| view.id.as_u64() > after)
            })
            .collect::<Vec<_>>();
        views.sort_by_key(|view| view.id.as_u64());
        let next_view = (views.len() > limit).then(|| views[limit - 1].id.as_u64());
        let views = views
            .into_iter()
            .take(limit)
            .map(|view| ViewInfo {
                id: view.id.as_u64(),
                document: view.doc.as_u64(),
                binding_revision: view.binding_revision(),
                selection_revision: self
                    .document(view.doc)
                    .and_then(|doc| doc.selection_revision(view.id))
                    .unwrap_or(0),
            })
            .collect();
        let mut catalog = StateCatalog {
            documents,
            views,
            next_document,
            next_view,
            error: None,
        };
        let mut context = EditorContext {
            generation: self.plugins.shared.generation,
            mode: self.mode.to_string(),
            ..EditorContext::default()
        };
        let requested = (|| -> anyhow::Result<()> {
            let view = query
                .view
                .map(|id| {
                    self.tree
                        .try_get(ViewId::from_u64(id))
                        .ok_or(PluginConflict::ViewClosed(id))
                })
                .transpose()?;
            let document = query
                .document
                .or_else(|| view.map(|view| view.doc.as_u64()));
            if let Some(id) = document {
                let doc = self
                    .documents
                    .values()
                    .find(|doc| doc.id().as_u64() == id)
                    .ok_or(PluginConflict::DocumentClosed(id))?;
                if let Some(view) = view {
                    ensure!(
                        view.doc == doc.id(),
                        "queried view is bound to another document"
                    );
                    context.view = bounded_view_snapshot(doc, view.id)?;
                }
                context.document = Some(snapshot(doc)?);
            }
            Ok(())
        })();
        if let Err(error) = requested {
            catalog.error = Some(error.to_string());
            context.document = None;
            context.view = None;
        }
        (context, catalog)
    }

    fn plugin_context_for_view(&self, origin: Option<ViewId>) -> anyhow::Result<EditorContext> {
        let view = origin.and_then(|id| self.tree.try_get(id));
        let doc = view.and_then(|view| self.document(view.doc));
        Ok(EditorContext {
            generation: self.plugins.shared.generation,
            mode: self.mode.to_string(),
            document: doc.map(snapshot).transpose()?,
            view: view
                .zip(doc)
                .map(|(view, doc)| bounded_view_snapshot(doc, view.id))
                .transpose()?
                .flatten(),
        })
    }

    fn plugin_context(&self) -> anyhow::Result<EditorContext> {
        self.plugin_context_for_view(Some(self.tree.focus))
    }

    fn plugin_invocation_target(&self, view: Option<ViewId>) -> InvocationTarget {
        let view = view.and_then(|view| self.tree.try_get(view));
        InvocationTarget {
            document: view.map(|view| view.doc.as_u64()),
            view: view.map(|view| view.id.as_u64()),
            binding_revision: view.map(|view| view.binding_revision()),
        }
    }

    fn plugin_global_context(&self) -> EditorContext {
        EditorContext {
            generation: self.plugins.shared.generation,
            mode: self.mode.to_string(),
            ..EditorContext::default()
        }
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
        self.prepare_plugin_reload(config, base)
    }

    fn refresh_plugin_subscriptions(&mut self) {
        *self.plugins.shared.readers.lock() = self
            .plugins
            .manager
            .event_recipients(Event::Init)
            .into_iter()
            .filter(|name| self.plugins.manager.can_read(name))
            .collect();
        *self.plugins.shared.subscriptions.lock() = OBSERVABLE_EVENTS
            .iter()
            .copied()
            .filter_map(|event| {
                let recipients = self
                    .plugins
                    .manager
                    .event_recipients(event)
                    .into_iter()
                    .collect::<HashSet<_>>();
                (!recipients.is_empty()).then_some((event, recipients))
            })
            .collect();
    }

    pub fn shutdown_plugins(&mut self) {
        self.begin_plugin_shutdown();
    }

    pub fn execute_plugin_command(
        &mut self,
        name: &str,
        args: Vec<String>,
    ) -> anyhow::Result<bool> {
        ensure!(
            !self.plugins.stopped && !self.plugins.shutting_down,
            "plugins have been shut down or are being replaced"
        );
        if self.plugin_command_doc(name).is_none() {
            return Ok(false);
        }
        let plugin = name.split_once('.').unwrap().0;
        let context = if self.plugins.manager.can_read(plugin) {
            self.plugin_context()?
        } else {
            self.plugin_global_context()
        };
        let provenance = self
            .plugins
            .shared
            .queue
            .lock()
            .provenance(self.plugins.shared.generation);
        let services = self.plugin_services(plugin, &provenance)?;
        let result = self.plugins.manager.call_command_with_target(
            name,
            args,
            context.clone(),
            with_provenance(Value::Null, &provenance),
            services,
            self.plugin_invocation_target(Some(self.tree.focus)),
        );
        self.refresh_plugin_subscriptions();
        let Some(response) = result? else {
            return Ok(false);
        };
        self.admit_plugin_call(plugin.into(), response, context, provenance, Event::Command)?;
        Ok(true)
    }

    pub fn dispatch_plugin_event(&mut self, event: Event, data: Value) -> bool {
        let capture = self
            .plugins
            .manager
            .event_recipients(event)
            .iter()
            .any(|name| self.plugins.manager.can_read(name));
        let context = match if capture {
            self.plugin_context()
        } else {
            Ok(self.plugin_global_context())
        } {
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
        let provenance = self
            .plugins
            .shared
            .queue
            .lock()
            .provenance(self.plugins.shared.generation);
        self.run_plugin_event(
            event,
            context,
            data,
            provenance,
            None,
            if matches!(event, Event::Init | Event::Shutdown) { InvocationTarget::default() } else { self.plugin_invocation_target(Some(self.tree.focus)) },
        )
    }

    /// Queue frontend events on the same owning-editor path as document hooks.
    pub fn queue_plugin_event(&self, event: Event, data: Value) {
        self.queue_plugin_event_for_view(event, self.tree.focus, data);
    }

    /// Captures the originating split rather than whichever split later gains focus.
    /// A closed origin still produces the event, with no document or view snapshot.
    pub fn queue_plugin_event_for_view(&self, event: Event, view: ViewId, data: Value) {
        self.queue_plugin_event_snapshot(
            event,
            view,
            data,
            self.plugin_invocation_target(Some(view)),
            || self.plugin_context_for_view(Some(view)),
        );
    }

    /// A delayed frontend completion retains its original document binding.
    /// Rebound/closed views omit the view snapshot; a still-open original
    /// document remains available even if hidden by another document meanwhile.
    pub fn queue_plugin_event_for_view_binding(
        &self,
        event: Event,
        view: ViewId,
        document: DocumentId,
        binding_revision: u64,
        data: Value,
    ) {
        let live_view = self
            .tree
            .try_get(view)
            .filter(|view| view.doc == document && view.binding_revision() == binding_revision);
        let scope = InvocationTarget {
            document: self.document(document).map(|doc| doc.id().as_u64()),
            view: live_view.map(|view| view.id.as_u64()),
            binding_revision: live_view.map(|view| view.binding_revision()),
        };
        self.queue_plugin_event_snapshot(event, view, data, scope, || {
            let doc = self.document(document);
            let view = self
                .tree
                .try_get(view)
                .filter(|view| view.doc == document && view.binding_revision() == binding_revision);
            Ok(EditorContext {
                generation: self.plugins.shared.generation,
                mode: self.mode.to_string(),
                document: doc.map(snapshot).transpose()?,
                view: view
                    .zip(doc)
                    .map(|(view, doc)| bounded_view_snapshot(doc, view.id))
                    .transpose()?
                    .flatten(),
            })
        });
    }

    pub fn plugin_event_source(&self) -> PluginEventSource {
        PluginEventSource {
            generation: self.plugins.shared.generation,
            origin: self.plugins.shared.queue.lock().origin.clone(),
        }
    }

    /// Queue a delayed native completion under its original plugin cause. A
    /// native invocation has no owner and may complete after a plugin reload.
    pub fn queue_plugin_event_for_view_binding_from_source(
        &self,
        source: &PluginEventSource,
        event: Event,
        view: ViewId,
        document: DocumentId,
        binding_revision: u64,
        data: Value,
    ) {
        if let Some(origin) = &source.origin
            && (source.generation != self.plugins.shared.generation
                || self
                    .plugins
                    .manager
                    .policy(&origin.plugin)
                    .is_none_or(|policy| policy.check_live().is_err()))
        {
            return;
        }
        let owner = self.plugins.shared.clone();
        let previous = std::mem::replace(&mut owner.queue.lock().origin, source.origin.clone());
        let _guard = ApplyingGuard { owner, previous };
        self.queue_plugin_event_for_view_binding(event, view, document, binding_revision, data);
    }

    fn queue_plugin_event_snapshot(
        &self,
        event: Event,
        view: ViewId,
        data: Value,
        scope: InvocationTarget,
        capture: impl FnOnce() -> anyhow::Result<EditorContext>,
    ) {
        let Some(sender) = self.plugins.sender(&self.handlers.callbacks) else {
            return;
        };
        if sender.interested(event) {
            match if sender.needs_context(event) {
                capture()
            } else {
                Ok(self.plugin_global_context())
            } {
                Ok(context) => sender.enqueue_scoped(event, context, data, None, scope),
                Err(error) => {
                    let mut data = match data {
                        Value::Object(data) => data,
                        _ => serde_json::Map::new(),
                    };
                    data.insert("snapshot_error".into(), Value::String(error.to_string()));
                    data.insert("source_view".into(), view.as_u64().into());
                    sender.enqueue_scoped(
                        event,
                        EditorContext {
                            generation: self.plugins.shared.generation,
                            mode: self.mode.to_string(),
                            ..EditorContext::default()
                        },
                        Value::Object(data),
                        None,
                        scope,
                    );
                }
            }
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

    /// Nonblocking editor-owned pump. It also recovers a wake rejected by a
    /// full callback destination; no task is spawned per event or retry.
    pub fn poll_plugin_events(&mut self) {
        self.poll_plugin_completions();
        let mut delivered_gap = false;
        let mut admission_blocked = false;
        for _ in 0..MAX_DRAIN_EVENTS {
            let pending = {
                let mut queue = self.plugins.shared.queue.lock();
                if !delivered_gap && queue.gap.is_some() {
                    delivered_gap = true;
                    let gap = queue.gap.take().unwrap();
                    let origin = queue.origin.take();
                    let provenance = queue.provenance(self.plugins.shared.generation);
                    queue.origin = origin;
                    Some(PendingEvent {
                        event: Event::ResyncRequired,
                        editor: EditorContext {
                            generation: self.plugins.shared.generation,
                            mode: self.mode.to_string(),
                            ..EditorContext::default()
                        },
                        data: serde_json::to_value(gap).unwrap(),
                        provenance,
                        target: None,
                        scope: InvocationTarget::default(),
                        bytes: 0,
                    })
                } else if !queue.pending.is_empty() {
                    Some(queue.remove(0))
                } else {
                    None
                }
            };
            let Some(mut pending) = pending else {
                break;
            };
            if pending.editor.mode.is_empty() {
                pending.editor.mode = self.mode.to_string();
            }
            let reliable = pending.target.is_some()
                && (frontend_reply(pending.event) || pending.event == Event::JobReady);
            let accepted = self.run_plugin_event(
                pending.event,
                pending.editor.clone(),
                pending.data.clone(),
                pending.provenance.clone(),
                pending.target.clone(),
                pending.scope,
            );
            if reliable && !accepted {
                let mut queue = self.plugins.shared.queue.lock();
                queue.bytes += pending.bytes;
                queue.pending.push_front(pending);
                admission_blocked = true;
                break;
            }
        }
        if !admission_blocked && let Some(sender) = self.plugins.sender(&self.handlers.callbacks) {
            sender.schedule_wake();
        }
    }

    fn run_plugin_event(
        &mut self,
        event: Event,
        context: EditorContext,
        data: Value,
        provenance: Provenance,
        target: Option<String>,
        scope: InvocationTarget,
    ) -> bool {
        let targeted = target.is_some();
        let names = target.map_or_else(
            || self.plugins.manager.event_recipients(event),
            |name| {
                self.plugins
                    .manager
                    .receives_event(&name, event)
                    .then_some(name)
                    .into_iter()
                    .collect()
            },
        );
        self.begin_plugin_batch();
        let mut successful = true;
        for name in names {
            if !targeted && provenance.origin_plugin.as_ref() == Some(&name) {
                continue;
            }
            let context = if self.plugins.manager.can_read(&name) {
                context.clone()
            } else {
                self.plugin_global_context()
            };
            let result = self
                .plugin_services(&name, &provenance)
                .and_then(|services| {
                    self.plugins.manager.call_event_with_target(
                        &name,
                        event,
                        context.clone(),
                        with_provenance(data.clone(), &provenance),
                        services,
                        scope,
                    )
                })
                .and_then(|completion| {
                    self.admit_plugin_call(
                        name.clone(),
                        completion,
                        context,
                        provenance.clone(),
                        event,
                    )
                });
            if let Err(err) = result {
                successful = false;
                log::error!("plugin '{name}': {err:#}");
                self.set_error(|| format!("plugin '{name}': {err:#}"));
                if !(targeted && (frontend_reply(event) || event == Event::JobReady)) {
                    self.plugins
                        .shared
                        .queue
                        .lock()
                        .lost(provenance.sequence, "worker-admission");
                }
                self.refresh_plugin_subscriptions();
            }
        }
        self.finish_plugin_batch();
        successful
    }

    pub(crate) fn queue_plugin_write_completed(&self, saved: &crate::document::DocumentSavedEvent) {
        let Some(sender) = self.plugins.sender(&self.handlers.callbacks) else {
            return;
        };
        if !sender.interested(Event::DocumentSaved) {
            return;
        }
        let document = Some(DocumentSnapshot {
            id: saved.doc_id.as_u64(),
            version: saved.version,
            path: Some(saved.path.to_string_lossy().into_owned()),
            language: self
                .document(saved.doc_id)
                .and_then(Document::language_name)
                .map(str::to_owned),
            char_count: saved.text.len_chars() as u64,
            byte_count: saved.text.len_bytes() as u64,
        });
        sender.enqueue(
            Event::DocumentSaved,
            EditorContext {
                generation: self.plugins.shared.generation,
                mode: self.mode.to_string(),
                document,
                view: None,
            },
            serde_json::json!({
                "document": saved.doc_id.as_u64(), "path": saved.path,
                "saved_revision": saved.revision, "saved_version": saved.version,
                "current_version": self.document(saved.doc_id).map(Document::version),
                "snapshot_available": true,
            }),
        );
    }

    fn apply_plugin_response(
        &mut self,
        response: Response,
        context: &EditorContext,
        plugin: &str,
        provenance: &Provenance,
        event: Event,
        opened_documents: Vec<crate::document::PreparedPluginDocument>,
    ) -> anyhow::Result<()> {
        let mut opened_documents = opened_documents.into_iter();
        if context.generation != self.plugins.shared.generation {
            return Err(PluginConflict::GenerationChanged.into());
        }
        if let Some(error) = response.error {
            bail!("{}", plugin_message(error))
        }
        // Every document/view precondition and every projected range is checked
        // before any effect is applied. Later edits use the preceding edit's text.
        let mut documents = BTreeMap::<DocumentId, PreparedDocument>::new();
        let mut prepared = Vec::new();
        let mut opened = false;
        for action in response.actions {
            if provenance.depth > MAX_CAUSAL_DEPTH
                && !matches!(action, Action::Status { .. } | Action::Error { .. })
            {
                return Err(ServiceError::new(
                    ErrorCode::ResourceExhausted,
                    "plugin causal effect limit exceeded",
                )
                .into());
            }
            if (self.plugins.stopped || self.plugins.shutting_down)
                && !matches!(action, Action::Status { .. } | Action::Error { .. })
            {
                return Err(ServiceError::new(
                    ErrorCode::Cancelled,
                    "plugin host is shutting down; only diagnostics are accepted",
                )
                .into());
            }
            if !matches!(
                action,
                Action::Status { .. } | Action::Error { .. } | Action::RequestState { .. }
            ) && let Some(origin) = context.view.as_ref()
                && self
                    .tree
                    .try_get(ViewId::from_u64(origin.id))
                    .is_none_or(|view| {
                        view.doc.as_u64() != origin.document
                            || view.binding_revision() != origin.binding_revision
                    })
            {
                return Err(PluginConflict::ViewRebound(origin.id).into());
            }
            for capability in action_capabilities(&action) {
                self.plugins
                    .manager
                    .require_capability(plugin, capability)?;
            }
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
                Action::Status { message } => {
                    prepared.push(PreparedAction::Status(plugin_message(message)))
                }
                Action::Error { message } => {
                    prepared.push(PreparedAction::Error(plugin_message(message)))
                }
                Action::RequestState { query } => {
                    ensure!(
                        self.plugins.shared.accepting.load(Ordering::Relaxed),
                        "plugin host is shutting down"
                    );
                    prepared.push(PreparedAction::State(query));
                }
                Action::Open { path } => {
                    opened = true;
                    ensure!(!path.is_empty(), "plugin open path is empty");
                    if let Some(origin) = &context.view {
                        let view = ViewId::from_u64(origin.id);
                        let target = self
                            .tree
                            .try_get(view)
                            .ok_or(PluginConflict::ViewClosed(origin.id))?;
                        if target.doc.as_u64() != origin.document
                            || target.binding_revision() != origin.binding_revision
                        {
                            return Err(PluginConflict::ViewRebound(origin.id).into());
                        }
                        let id = self.plugin_document(
                            origin.document,
                            context
                                .document
                                .as_ref()
                                .ok_or_else(|| {
                                    ServiceError::new(
                                        ErrorCode::StaleState,
                                        "plugin open has no originating document",
                                    )
                                })?
                                .version,
                        )?;
                        if self.documents[&id].selection_revision(view)
                            != Some(origin.selection_revision)
                        {
                            return Err(PluginConflict::SelectionChanged(origin.id).into());
                        }
                        if self.tree.focus != view {
                            return Err(ServiceError::new(
                                ErrorCode::StaleState,
                                "plugin open origin is no longer focused",
                            )
                            .into());
                        }
                    } else if self.tree.views().next().is_some() {
                        self.plugins
                            .manager
                            .require_capability(plugin, Capability::EditorRead)?;
                        return Err(ServiceError::new(
                            ErrorCode::StaleState,
                            "plugin open has no originating view",
                        )
                        .into());
                    } else if event != Event::Init {
                        return Err(ServiceError::new(
                            ErrorCode::StaleState,
                            "plugin open cannot recreate a closed editor",
                        )
                        .into());
                    }
                    let doc = opened_documents.next().ok_or_else(|| {
                        ServiceError::new(
                            ErrorCode::HostFailure,
                            "plugin open was not prepared off-thread",
                        )
                    })?;
                    let doc = Document::from_prepared_plugin(
                        doc,
                        self.config.clone(),
                        self.syn_loader.clone(),
                    );
                    prepared.push(PreparedAction::Open(Box::new(doc)));
                }
                Action::ShowUi { .. }
                | Action::InvokeBuiltin { .. }
                | Action::UpdateKeymap { .. } => {
                    return Err(ServiceError::new(
                        ErrorCode::UnsupportedInterface,
                        "native frontend services are unavailable",
                    )
                    .into());
                }
            }
        }
        let owner = self.plugins.shared.clone();
        let previous = owner.queue.lock().origin.replace(EffectOrigin {
            plugin: plugin.into(),
            sequence: provenance.sequence,
            depth: provenance.depth,
        });
        let _guard = ApplyingGuard { owner, previous };
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
                PreparedAction::State(query) => {
                    let (editor, catalog) = self.plugin_state(&query);
                    if let Some(sender) = self.plugins.sender(&self.handlers.callbacks) {
                        sender.enqueue_target(
                            Event::State,
                            editor,
                            serde_json::to_value(catalog).unwrap(),
                            Some(plugin.into()),
                        );
                    }
                }
                PreparedAction::Status(message) => self.set_status(message),
                PreparedAction::Error(message) => self.set_error(|| message),
                PreparedAction::Open(doc) => {
                    let action = if self.tree.views().next().is_none() {
                        crate::editor::Action::VerticalSplit
                    } else {
                        crate::editor::Action::Replace
                    };
                    self.adopt_plugin_document(*doc, action);
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

struct ApplyingGuard {
    owner: Arc<Shared>,
    previous: Option<EffectOrigin>,
}
impl Drop for ApplyingGuard {
    fn drop(&mut self) {
        self.owner.queue.lock().origin = self.previous.take();
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
    State(StateQuery),
    Status(String),
    Error(String),
    Open(Box<Document>),
}

fn action_capabilities(action: &Action) -> Vec<Capability> {
    match action {
        Action::Edit { .. } => vec![Capability::EditorEdit],
        Action::SetSelection { .. } => vec![Capability::EditorSelection],
        Action::Status { .. }
        | Action::Error { .. }
        | Action::ShowUi { .. }
        | Action::UpdateKeymap { .. } => vec![Capability::Ui],
        Action::Open { .. } => vec![Capability::EditorNavigate, Capability::WorkspaceRead],
        Action::RequestState { .. } => vec![Capability::EditorRead],
        Action::InvokeBuiltin { commands, .. } => commands
            .iter()
            .flat_map(|command| command.command.capabilities().iter().copied())
            .collect(),
    }
}

fn plugin_message(mut message: String) -> String {
    let mut end = message.len().min(MAX_STATUS_BYTES);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
    plugin_api::ui::terminal_text(&message, false)
}

pub(crate) fn register_hooks() {
    use crate::events::{
        DocumentDidChange, DocumentDidClose, DocumentDidOpen, DocumentFocusLost, SelectionDidChange,
    };
    event::register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        let editor = &event.editor;
        if let Some(sender) = editor.plugins.sender(&editor.handlers.callbacks)
            && sender.interested(Event::DocumentFocusLost)
        {
            let document = editor
                .document(event.doc)
                .map(snapshot)
                .transpose()
                .ok()
                .flatten();
            let view = editor
                .document(event.doc)
                .and_then(|doc| view_snapshot(doc, event.view));
            sender.enqueue(
                Event::DocumentFocusLost,
                EditorContext {
                    generation: editor.plugins.shared.generation,
                    mode: editor.mode.to_string(),
                    document,
                    view,
                },
                serde_json::json!({"document": event.doc.as_u64(), "view": event.view.as_u64()}),
            );
        }
        Ok(())
    });
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

//! Editor-owned readiness polling. Polling here only receives worker results;
//! compilation, guest execution, filesystem access and text copying stay off-thread.
use super::*;
use crate::callbacks::{InvocationTask, InvocationTasks, TaskOutcome};
use ::plugins::{
    native::{NativeBudget, NativeServices},
    CompletedResponse, Completion, ManagerPreparation, PreparedManager,
};
use futures_util::{future::BoxFuture, stream::FuturesUnordered, Stream};
use plugin_api::{HostFuture, HostServices, ReadRequest};
use std::{
    future::Future,
    pin::Pin,
    task::{Context as TaskContext, Poll, Wake, Waker},
    time::Duration,
};

const MAX_CALLS: usize = 64;
const DEADLINE: Duration = Duration::from_secs(6);

struct Ready {
    plugin: String,
    context: EditorContext,
    provenance: Provenance,
    event: Event,
    result: Result<CompletedResponse, ServiceError>,
    opened: Vec<crate::document::PreparedPluginDocument>,
    _open_quota: Option<tokio::sync::OwnedSemaphorePermit>,
    observer: Option<Arc<dyn InvocationTasks>>,
    task: Option<InvocationTask>,
}
struct Loading {
    future: ManagerPreparation,
    observer: Option<Arc<dyn InvocationTasks>>,
    task: Option<InvocationTask>,
}
struct Replacement {
    prepared: PreparedManager,
    observer: Option<Arc<dyn InvocationTasks>>,
    task: Option<InvocationTask>,
    shutdown_sent: bool,
}
pub(super) struct AsyncState {
    calls: FuturesUnordered<BoxFuture<'static, Vec<Ready>>>,
    staging: Option<Vec<BoxFuture<'static, Ready>>>,
    call_count: usize,
    loading: Option<Loading>,
    replacement: Option<Replacement>,
    cleanup: FuturesUnordered<BoxFuture<'static, Result<(), ServiceError>>>,
    services: BTreeMap<String, Arc<NativeServices>>,
    pub(super) budget: NativeBudget,
    reads: Arc<tokio::sync::Semaphore>,
    open_bytes: Arc<tokio::sync::Semaphore>,
    shutdown_sent: bool,
}

impl Default for AsyncState {
    fn default() -> Self {
        Self {
            calls: FuturesUnordered::new(),
            staging: None,
            call_count: 0,
            loading: None,
            replacement: None,
            cleanup: FuturesUnordered::new(),
            services: BTreeMap::new(),
            budget: NativeBudget::default(),
            reads: Arc::new(tokio::sync::Semaphore::new(1)),
            open_bytes: Arc::new(tokio::sync::Semaphore::new(32 * 1024 * 1024)),
            shutdown_sent: false,
        }
    }
}

struct EditorWake(PluginEventSender);
impl Wake for EditorWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(owner) = self.0.owner.upgrade() {
            owner.async_woken.store(true, Ordering::Release);
            self.0.schedule_wake();
        }
    }
}

impl Editor {
    pub(crate) fn cancel_plugin_target(&self, document: Option<u64>, view: Option<u64>) {
        self.plugins.manager.cancel_target(document, view);
    }

    fn plugin_waker(&self) -> Waker {
        Waker::from(Arc::new(EditorWake(PluginEventSender {
            owner: Arc::downgrade(&self.plugins.shared),
            callbacks: self.handlers.callbacks.clone(),
        })))
    }

    pub(super) fn prepare_plugin_reload(
        &mut self,
        config: &BTreeMap<String, PluginConfig>,
        base: &Path,
    ) -> bool {
        if self.plugins.shutting_down
            || self.plugins.stopped
            || self.plugins.asynchronous.loading.is_some()
            || self.plugins.asynchronous.replacement.is_some()
            || !self.plugins.asynchronous.cleanup.is_empty()
        {
            self.set_error(|| "plugin replacement is already pending or the editor is closing");
            return false;
        }
        let Some(generation) = self.plugins.shared.generation.checked_add(1) else {
            self.set_error(|| "plugin generation exhausted");
            return false;
        };
        match self
            .plugins
            .manager
            .prepare(config.clone(), base.to_owned(), generation)
        {
            Ok(future) => {
                let observer = self.invocation_tasks();
                let task = InvocationTask::new_named(observer.clone(), "plugin reload cancelled");
                self.plugins.asynchronous.loading = Some(Loading {
                    future,
                    observer,
                    task,
                });
                self.poll_plugin_events();
                true
            }
            Err(error) => {
                self.report_plugin_error("loader", &error);
                false
            }
        }
    }

    pub(super) fn plugin_services(
        &mut self,
        plugin: &str,
        provenance: &Provenance,
    ) -> Result<Arc<dyn HostServices>, ServiceError> {
        let policy = self.plugins.manager.policy(plugin).ok_or_else(cancelled)?;
        let adapter: Arc<dyn HostServices> = Arc::new(EditorServices {
            owner: Arc::downgrade(&self.plugins.shared),
            callbacks: self.handlers.callbacks.clone(),
            policy: policy.clone(),
            plugin: plugin.into(),
            reads: self.plugins.asynchronous.reads.clone(),
            source: PluginEventSource {
                generation: self.plugins.shared.generation,
                origin: Some(EffectOrigin {
                    plugin: plugin.into(),
                    sequence: provenance.sequence,
                    depth: provenance.depth,
                }),
            },
        });
        let native = if let Some(native) = self.plugins.asynchronous.services.get(plugin) {
            native.clone()
        } else {
            let path = loader::data_dir().join("plugins").join(plugin);
            let native = Arc::new(NativeServices::with_budget(
                policy,
                adapter.clone(),
                Some(&path),
                self.plugins.asynchronous.budget.clone(),
            )?);
            self.plugins
                .asynchronous
                .services
                .insert(plugin.into(), native.clone());
            native
        };
        Ok(Arc::new(native.with_editor(adapter)))
    }

    pub(super) fn admit_plugin_call(
        &mut self,
        plugin: String,
        completion: Completion,
        context: EditorContext,
        provenance: Provenance,
        event: Event,
    ) -> Result<(), ServiceError> {
        if self.plugins.asynchronous.call_count >= MAX_CALLS
            || (event != Event::Command && self.plugins.asynchronous.call_count >= MAX_CALLS - 8)
        {
            return Err(ServiceError::new(
                ErrorCode::ResourceExhausted,
                "editor plugin completion queue is full",
            ));
        }
        let observer = if matches!(event, Event::Command | Event::Init) {
            self.invocation_tasks()
        } else {
            None
        };
        let task = InvocationTask::new_named(observer.clone(), "plugin invocation cancelled");
        let policy = self.plugins.manager.policy(&plugin).ok_or_else(cancelled)?;
        let reads = self.plugins.asynchronous.reads.clone();
        let open_bytes = self.plugins.asynchronous.open_bytes.clone();
        let future: BoxFuture<'static, Ready> = Box::pin(async move {
            let result = completion.await;
            let mut opened = Vec::new();
            let mut open_quota = None;
            let result = match result {
                Ok(completed) => {
                    let paths = completed
                        .actions
                        .iter()
                        .filter_map(|action| match action {
                            Action::Open { path } => Some(path.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    if paths.is_empty() {
                        Ok(completed)
                    } else {
                        let prepared = async {
                            policy.require(Capability::EditorNavigate)?;
                            policy.require(Capability::WorkspaceRead)?;
                            // Reserve before allocation. The blocking task owns both
                            // permits if the invocation is cancelled while I/O runs.
                            let quota = open_bytes
                                .acquire_many_owned(::plugins::MAX_MESSAGE_BYTES as u32)
                                .await
                                .map_err(|_| cancelled())?;
                            let permit = reads.acquire_owned().await.map_err(|_| cancelled())?;
                            tokio::task::spawn_blocking(move || {
                                let _permit = permit;
                                let mut docs = Vec::new();
                                let mut total = 0usize;
                                let mut decoded = 0usize;
                                for path in paths {
                                    let (path, bytes) = policy.read_path(Path::new(&path))?;
                                    total = total.saturating_add(bytes.len());
                                    if total > ::plugins::MAX_MESSAGE_BYTES {
                                        return Err(ServiceError::new(
                                            ErrorCode::ResourceExhausted,
                                            "plugin open contents exceed the response budget",
                                        ));
                                    }
                                    let doc = Document::prepare_plugin_bytes(
                                        path,
                                        bytes,
                                        editor_core::editor_config::EditorConfig::default(),
                                    )
                                    .map_err(|cause| {
                                        ServiceError::new(
                                            ErrorCode::InvalidRequest,
                                            cause.to_string(),
                                        )
                                    })?;
                                    if doc.binary {
                                        return Err(ServiceError::new(
                                            ErrorCode::InvalidRequest,
                                            "plugin open only supports text documents",
                                        ));
                                    }
                                    decoded = decoded.saturating_add(doc.byte_count());
                                    if decoded > ::plugins::MAX_MESSAGE_BYTES { return Err(ServiceError::new(ErrorCode::ResourceExhausted, "decoded plugin open contents exceed the response budget")); }
                                    docs.push(doc);
                                }
                                Ok((docs,quota))
                            })
                            .await
                            .map_err(|cause| {
                                ServiceError::new(ErrorCode::HostFailure, cause.to_string())
                            })?
                        };
                        let prepared = tokio::time::timeout(DEADLINE, prepared)
                            .await
                            .unwrap_or_else(|_| {
                                Err(ServiceError::new(
                                    ErrorCode::DeadlineExceeded,
                                    "plugin open preparation timed out",
                                ))
                            });
                        match prepared {
                            Ok((docs, quota)) => {
                                opened = docs;
                                open_quota = Some(quota);
                                Ok(completed)
                            }
                            Err(error) => Err(error),
                        }
                    }
                }
                Err(error) => Err(error),
            };
            Ready {
                result,
                opened,
                _open_quota: open_quota,
                plugin,
                context,
                provenance,
                event,
                observer,
                task,
            }
        });
        self.plugins.asynchronous.call_count += 1;
        if let Some(staging) = self.plugins.asynchronous.staging.as_mut() {
            staging.push(future);
        } else {
            self.plugins
                .asynchronous
                .calls
                .push(Box::pin(async move { vec![future.await] }));
        }

        // Register the owning-editor wake immediately, without polling a guest.
        if self.plugins.asynchronous.staging.is_none() {
            self.poll_plugin_completions();
        }
        Ok(())
    }

    pub(super) fn begin_plugin_batch(&mut self) {
        self.plugins.asynchronous.staging = Some(Vec::new());
    }
    pub(super) fn finish_plugin_batch(&mut self) {
        let batch = self.plugins.asynchronous.staging.take().unwrap();
        if !batch.is_empty() {
            self.plugins
                .asynchronous
                .calls
                .push(Box::pin(futures_util::future::join_all(batch)));
        }
        self.poll_plugin_completions();
    }

    fn report_plugin_error(&mut self, plugin: &str, error: &impl std::fmt::Display) {
        let message = plugin_message(format!("plugin '{plugin}': {error}"));
        log::error!("{message}");
        self.set_error(|| message);
    }

    pub(super) fn poll_plugin_completions(&mut self) {
        let waker = self.plugin_waker();
        let mut cx = TaskContext::from_waker(&waker);
        for _ in 0..MAX_CALLS {
            let Poll::Ready(Some(batch)) =
                Pin::new(&mut self.plugins.asynchronous.calls).poll_next(&mut cx)
            else {
                break;
            };
            self.plugins.asynchronous.call_count -= batch.len();
            for ready in batch {
                let previous = self.replace_invocation_tasks(ready.observer);
                let result = ready
                    .result
                    .map_err(anyhow::Error::from)
                    .and_then(|completed| {
                        completed.with_response(|response| {
                            self.apply_plugin_response(
                                response,
                                &ready.context,
                                &ready.plugin,
                                &ready.provenance,
                                ready.event,
                                ready.opened,
                            )
                        })
                    });
                self.replace_invocation_tasks(previous);
                let outcome = match result {
                    Ok(()) => TaskOutcome::Success,
                    Err(error) => {
                        self.report_plugin_error(&ready.plugin, &error);
                        if error
                            .downcast_ref::<ServiceError>()
                            .is_some_and(|e| e.code == ErrorCode::Cancelled)
                        {
                            TaskOutcome::Cancelled(error.to_string())
                        } else {
                            TaskOutcome::Error(error.to_string())
                        }
                    }
                };
                if let Some(task) = ready.task {
                    task.finish(outcome);
                }
                self.refresh_plugin_subscriptions();
            }
        }
        while let Poll::Ready(Some(result)) =
            Pin::new(&mut self.plugins.asynchronous.cleanup).poll_next(&mut cx)
        {
            if let Err(error) = result {
                self.report_plugin_error("cleanup", &error);
            }
        }
        if let Some(loading) = self.plugins.asynchronous.loading.as_mut()
            && let Poll::Ready(result) = Pin::new(&mut loading.future).poll(&mut cx)
        {
            let loading = self.plugins.asynchronous.loading.take().unwrap();
            match result {
                Ok(prepared) => {
                    self.plugins.asynchronous.replacement = Some(Replacement {
                        prepared,
                        observer: loading.observer,
                        task: loading.task,
                        shutdown_sent: false,
                    });
                    self.plugins.shutting_down = true;
                    self.plugins
                        .shared
                        .accepting
                        .store(false, Ordering::Release);
                    // Replacing a generation cancels accepted old invocations;
                    // queued lifecycle hooks still drain before Shutdown.
                    self.plugins.asynchronous.calls.clear();
                    self.plugins.asynchronous.call_count = 0;
                }
                Err(error) => {
                    if let Some(task) = loading.task {
                        task.finish(TaskOutcome::Error(error.to_string()));
                    }
                    self.report_plugin_error("loader", &error);
                }
            }
        }
        self.advance_plugin_replacement();
        self.plugins
            .shared
            .async_pending
            .store(self.has_pending_plugin_work(), Ordering::Release);
    }

    fn advance_plugin_replacement(&mut self) {
        if self.plugins.asynchronous.replacement.is_none()
            || !self.plugins.asynchronous.calls.is_empty()
        {
            return;
        }
        let queue = self.plugins.shared.queue.lock();
        if !queue.pending.is_empty() || queue.gap.is_some() {
            return;
        }
        drop(queue);
        if !self
            .plugins
            .asynchronous
            .replacement
            .as_ref()
            .unwrap()
            .shutdown_sent
        {
            self.plugins
                .asynchronous
                .replacement
                .as_mut()
                .unwrap()
                .shutdown_sent = true;
            self.dispatch_plugin_event(Event::Shutdown, Value::Null);
            if !self.plugins.asynchronous.calls.is_empty() {
                return;
            }
        }
        let replacement = self.plugins.asynchronous.replacement.take().unwrap();
        let manager = match replacement.prepared.activate() {
            Ok(manager) => manager,
            Err(error) => {
                self.plugins.shutting_down = false;
                self.plugins.shared.accepting.store(true, Ordering::Release);
                if let Some(task) = replacement.task {
                    task.finish(TaskOutcome::Error(error.to_string()));
                }
                self.report_plugin_error("loader", &error);
                return;
            }
        };
        let old = std::mem::replace(&mut self.plugins.manager, manager);
        old.revoke();
        self.plugins
            .asynchronous
            .cleanup
            .push(Box::pin(async move { old.shutdown().await }));
        self.plugins.asynchronous.services.clear();
        self.plugins
            .shared
            .accepting
            .store(false, Ordering::Release);
        self.plugins.shared = Arc::new(Shared {
            generation: self.plugins.manager.generation(),
            accepting: AtomicBool::new(true),
            ..Shared::default()
        });
        self.plugins.shutting_down = false;
        self.plugins.stopped = false;
        self.refresh_plugin_subscriptions();
        let sender = self.plugins.sender(&self.handlers.callbacks);
        for doc in self.documents.values_mut() {
            doc.plugin_events = sender.clone();
        }
        let previous = self.replace_invocation_tasks(replacement.observer);
        let accepted = self.dispatch_plugin_event(Event::Init, Value::Null);
        self.replace_invocation_tasks(previous);
        self.plugins
            .shared
            .async_woken
            .store(true, Ordering::Release);
        PluginEventSender {
            owner: Arc::downgrade(&self.plugins.shared),
            callbacks: self.handlers.callbacks.clone(),
        }
        .schedule_wake();
        if let Some(task) = replacement.task {
            task.finish(if accepted {
                TaskOutcome::Success
            } else {
                TaskOutcome::Error("plugin initialization could not be admitted".into())
            });
        }
    }

    /// Readiness includes worker results, package preparation and bounded hooks.
    pub fn has_pending_plugin_work(&self) -> bool {
        let queue = self.plugins.shared.queue.lock();
        !self.plugins.asynchronous.calls.is_empty()
            || self.plugins.asynchronous.loading.is_some()
            || self.plugins.asynchronous.replacement.is_some()
            || !self.plugins.asynchronous.cleanup.is_empty()
            || !queue.pending.is_empty()
            || queue.gap.is_some()
    }

    pub(super) fn begin_plugin_shutdown(&mut self) {
        if self.plugins.stopped {
            return;
        }
        self.plugins.shutting_down = true;
        self.plugins.asynchronous.loading = None;
        self.plugins.asynchronous.replacement = None;
        self.poll_plugin_events();
    }

    /// Drain accepted lifecycle notifications before shutdown, then await actual
    /// actor/native-child cleanup. Guest execution is never polled on this thread.
    pub async fn finish_plugin_shutdown(&mut self) {
        self.begin_plugin_shutdown();
        if self.plugins.stopped {
            return;
        }
        let mut deadline = tokio::time::Instant::now() + DEADLINE;
        loop {
            self.poll_plugin_events();
            let empty = {
                let queue = self.plugins.shared.queue.lock();
                queue.pending.is_empty() && queue.gap.is_none()
            };
            if empty && self.plugins.asynchronous.calls.is_empty() {
                if !self.plugins.asynchronous.shutdown_sent {
                    self.plugins.asynchronous.shutdown_sent = true;
                    self.dispatch_plugin_event(Event::Shutdown, Value::Null);
                } else {
                    break;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                self.report_plugin_error(
                    "shutdown",
                    &"plugin drain deadline exceeded; remaining invocations cancelled",
                );
                if !self.plugins.asynchronous.shutdown_sent {
                    self.plugins.asynchronous.calls.clear();
                    self.plugins.asynchronous.call_count = 0;
                    let provenance = {
                        let mut queue = self.plugins.shared.queue.lock();
                        while !queue.pending.is_empty() {
                            let event = queue.remove(0);
                            queue.lost(event.provenance.sequence, "shutdown-budget");
                        }
                        queue.gap = None;
                        queue.provenance(self.plugins.shared.generation)
                    };
                    self.run_plugin_event(
                        Event::ResyncRequired,
                        self.plugin_global_context(),
                        serde_json::json!({"closing":true,"reason":"shutdown-budget"}),
                        provenance,
                        None,
                        InvocationTarget::default(),
                    );
                    self.plugins.asynchronous.shutdown_sent = true;
                    self.dispatch_plugin_event(Event::Shutdown, Value::Null);
                    deadline = tokio::time::Instant::now() + DEADLINE;
                } else {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        self.plugins
            .shared
            .accepting
            .store(false, Ordering::Release);
        self.plugins.asynchronous.calls.clear();
        self.plugins.asynchronous.call_count = 0;
        self.plugins.shared.queue.lock().pending.clear();
        self.plugins.shared.queue.lock().bytes = 0;
        if let Err(error) = self.plugins.manager.shutdown().await {
            self.report_plugin_error("shutdown", &error);
        }
        self.plugins.asynchronous.services.clear();
        self.plugins.stopped = true;
        for doc in self.documents.values_mut() {
            doc.plugin_events = None;
        }
    }
}

fn cancelled() -> ServiceError {
    ServiceError::new(
        ErrorCode::Cancelled,
        "plugin generation is no longer active",
    )
}
fn stale(message: &str) -> ServiceError {
    ServiceError::new(ErrorCode::StaleState, message)
}
struct EditorServices {
    owner: Weak<Shared>,
    callbacks: EditorCallbackSender,
    policy: Arc<::plugins::policy::AccessPolicy>,
    plugin: String,
    reads: Arc<tokio::sync::Semaphore>,
    source: PluginEventSource,
}
impl HostServices for EditorServices {
    fn notify_job_ready(&self, job: u64) -> HostFuture<()> {
        let weak = self.owner.clone();
        let callbacks = self.callbacks.clone();
        let policy = self.policy.clone();
        let plugin = self.plugin.clone();
        Box::pin(async move {
            policy.check_live()?;
            let (send, receive) = tokio::sync::oneshot::channel();
            callbacks
                .send(move |editor| {
                    let result = (|| {
                        let owner = weak.upgrade().ok_or_else(cancelled)?;
                        if !Arc::ptr_eq(&owner, &editor.plugins.shared)
                            || !owner.accepting.load(Ordering::Acquire)
                            || editor.plugins.shutting_down
                        {
                            return Err(cancelled());
                        }
                        let scope = editor
                            .plugins
                            .manager
                            .job_target(&plugin, job)
                            .ok_or_else(cancelled)?;
                        if scope.document.is_some_and(|id| {
                            editor.documents.values().all(|doc| doc.id().as_u64() != id)
                        }) || scope.view.is_some_and(|id| {
                            editor
                                .tree
                                .try_get(ViewId::from_u64(id))
                                .is_none_or(|view| {
                                    scope.document != Some(view.doc.as_u64())
                                        || scope.binding_revision != Some(view.binding_revision())
                                })
                        }) {
                            return Err(cancelled());
                        }
                        let context = if editor.plugins.manager.can_read(&plugin) {
                            let query = StateQuery {
                                document: scope.document,
                                view: scope.view,
                                ..StateQuery::default()
                            };
                            editor.plugin_state(&query).0
                        } else {
                            editor.plugin_global_context()
                        };
                        let sender = PluginEventSender {
                            owner: Arc::downgrade(&owner),
                            callbacks: editor.handlers.callbacks.clone(),
                        };
                        let origin = owner.queue.lock().origin.take();
                        sender.enqueue_scoped(
                            Event::JobReady,
                            context,
                            serde_json::json!({"job":job}),
                            Some(plugin),
                            scope,
                        );
                        owner.queue.lock().origin = origin;
                        Ok(())
                    })();
                    let _ = send.send(result);
                })
                .await;
            receive.await.map_err(|_| cancelled())?
        })
    }

    fn read_document(&self, request: ReadRequest) -> HostFuture<String> {
        let owner = self.owner.clone();
        let callbacks = self.callbacks.clone();
        let policy = self.policy.clone();
        let reads = self.reads.clone();
        let generation = self.source.generation;
        Box::pin(async move {
            policy.require(Capability::EditorRead)?;
            let shared = owner.upgrade().ok_or_else(cancelled)?;
            if shared.generation != generation
                || shared.generation != request.generation
                || !shared.accepting.load(Ordering::Acquire)
            {
                return Err(cancelled());
            }
            if request.start > request.end || request.max_bytes > ::plugins::MAX_MESSAGE_BYTES {
                return Err(ServiceError::new(
                    ErrorCode::InvalidRequest,
                    "invalid or oversized document read",
                ));
            }
            let permit = reads.acquire_owned().await.map_err(|_| cancelled())?;
            let (send, receive) = tokio::sync::oneshot::channel();
            let weak = owner.clone();
            callbacks
                .send(move |editor| {
                    let result = (|| {
                        let shared = weak.upgrade().ok_or_else(cancelled)?;
                        if !Arc::ptr_eq(&shared, &editor.plugins.shared) || editor.plugins.stopped {
                            return Err(cancelled());
                        }
                        let doc = editor
                            .documents
                            .values()
                            .find(|doc| doc.id().as_u64() == request.document)
                            .ok_or_else(|| stale("document is closed"))?;
                        // One editor-wide permit spans captures/materialization and
                        // replacement generations. The retained source is capped too:
                        // a tiny region cannot pin an arbitrarily large old document.
                        if doc.text().len_bytes() > 128 * 1024 * 1024 {
                            return Err(ServiceError::new(
                                ErrorCode::ResourceExhausted,
                                "document source exceeds the 128 MiB retained snapshot limit",
                            ));
                        }
                        if doc.version() != request.version {
                            return Err(stale("document version changed"));
                        }
                        let start = usize::try_from(request.start)
                            .map_err(|_| stale("read offset is out of bounds"))?;
                        let end = usize::try_from(request.end)
                            .map_err(|_| stale("read offset is out of bounds"))?;
                        if end > doc.text().len_chars() {
                            return Err(stale("read range is out of bounds"));
                        }
                        if doc.text().char_to_byte(end) - doc.text().char_to_byte(start)
                            > request.max_bytes
                        {
                            return Err(ServiceError::new(
                                ErrorCode::ResourceExhausted,
                                "document region exceeds requested byte limit",
                            ));
                        }
                        Ok((doc.text().clone(), start, end))
                    })();
                    let _ = send.send(result);
                })
                .await;
            let (text, start, end) = tokio::time::timeout(DEADLINE, receive)
                .await
                .map_err(|_| {
                    ServiceError::new(ErrorCode::DeadlineExceeded, "document capture timed out")
                })?
                .map_err(|_| cancelled())??;
            let value = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                text.slice(start..end).to_string()
            })
            .await
            .map_err(|cause| ServiceError::new(ErrorCode::HostFailure, cause.to_string()))?;
            policy.require(Capability::EditorRead)?;
            if !shared.accepting.load(Ordering::Acquire) {
                return Err(cancelled());
            }
            Ok(value)
        })
    }
}

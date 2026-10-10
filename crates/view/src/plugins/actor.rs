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
const MAX_ASSET_RETRIES: u8 = 3;
const MAX_READ_CAPTURES: usize = 8;

type CapturedRegion = (editor_core::Rope, usize, usize);

struct ReadCapture {
    request: ReadRequest,
    policy: Arc<::plugins::policy::AccessPolicy>,
    reply: tokio::sync::oneshot::Sender<Result<CapturedRegion, ServiceError>>,
}

/// Only bounded read metadata crosses this queue. It remains pumpable after a
/// frontend closes its generic callback channel, without admitting mutations.
#[derive(Default)]
pub(super) struct ReadCaptures {
    pending: Mutex<VecDeque<ReadCapture>>,
    closed: AtomicBool,
}

impl ReadCaptures {
    fn enqueue(&self, capture: ReadCapture) -> Result<(), ServiceError> {
        let mut pending = self.pending.lock();
        if self.closed.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        pending.retain(|capture| !capture.reply.is_closed());
        if pending.len() >= MAX_READ_CAPTURES {
            return Err(ServiceError::new(
                ErrorCode::ResourceExhausted,
                "editor document capture queue is full",
            ));
        }
        pending.push_back(capture);
        Ok(())
    }

    fn cancel(&self) {
        let mut pending = self.pending.lock();
        self.closed.store(true, Ordering::Release);
        for capture in pending.drain(..) {
            let _ = capture.reply.send(Err(cancelled()));
        }
    }
}

struct Ready {
    plugin: String,
    context: EditorContext,
    provenance: Provenance,
    event: Event,
    result: Result<CompletedResponse, ServiceError>,
    worker_succeeded: bool,
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
struct AssetLoading {
    prepared: PreparedManager,
    future: BoxFuture<'static, Result<assets::PreparedAssets, ServiceError>>,
    transition: AssetTransition,
}
struct AssetTransition {
    observer: Option<Arc<dyn InvocationTasks>>,
    task: Option<InvocationTask>,
    retiring: bool,
    shutdown_sent: bool,
    attempts: u8,
}
struct Replacement {
    prepared: PreparedManager,
    assets: Option<assets::PreparedAssets>,
    observer: Option<Arc<dyn InvocationTasks>>,
    task: Option<InvocationTask>,
    shutdown_sent: bool,
    asset_attempts: u8,
}
pub(super) struct AsyncState {
    calls: FuturesUnordered<BoxFuture<'static, Vec<Ready>>>,
    staging: Option<Vec<BoxFuture<'static, Ready>>>,
    call_count: usize,
    navigation: BTreeMap<(String, u64), EditorContext>,
    loading: Option<Loading>,
    asset_loading: Option<AssetLoading>,
    replacement: Option<Replacement>,
    advancing_replacement: bool,
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
            navigation: BTreeMap::new(),
            loading: None,
            asset_loading: None,
            replacement: None,
            advancing_replacement: false,
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
    pub(super) fn poll_plugin_read_captures(&self) {
        for _ in 0..MAX_READ_CAPTURES {
            let Some(capture) = self.plugins.shared.captures.pending.lock().pop_front() else {
                break;
            };
            if capture.reply.is_closed() {
                continue;
            }
            let request = capture.request;
            let result = (|| {
                capture.policy.require(Capability::EditorRead)?;
                if self.plugins.stopped || request.generation != self.plugins.shared.generation {
                    return Err(cancelled());
                }
                if request.start > request.end || request.max_bytes > ::plugins::MAX_MESSAGE_BYTES {
                    return Err(ServiceError::new(
                        ErrorCode::InvalidRequest,
                        "invalid or oversized document read",
                    ));
                }
                let doc = self
                    .documents
                    .values()
                    .find(|doc| doc.id().as_u64() == request.document)
                    .ok_or_else(|| stale("document is closed"))?;
                // The shared permit spans capture/materialization, including
                // close and replacement. Bound the retained source as well as
                // its requested region before cloning the Rope.
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
                if doc.text().char_to_byte(end) - doc.text().char_to_byte(start) > request.max_bytes
                {
                    return Err(ServiceError::new(
                        ErrorCode::ResourceExhausted,
                        "document region exceeds requested byte limit",
                    ));
                }
                Ok((doc.text().clone(), start, end))
            })();
            let _ = capture.reply.send(result);
        }
    }

    pub(crate) fn cancel_plugin_target(&self, document: Option<u64>, view: Option<u64>) {
        if let Some(origin) = self.plugins.shared.queue.lock().origin.as_ref() {
            self.plugins.manager.cancel_target_except(
                document,
                view,
                &origin.plugin,
                origin.sequence,
            );
        } else {
            self.plugins.manager.cancel_target(document, view);
        }
    }

    pub(super) fn record_plugin_navigation(
        &mut self,
        source: &PluginEventSource,
        reply: &plugin_api::editor::EditorReply,
    ) {
        let Some(origin) = source.origin.as_ref() else {
            return;
        };
        let context = if let plugin_api::editor::EditorReply::View { target } = reply {
            let view = self.tree.try_get(ViewId::from_u64(target.view));
            let doc = view.and_then(|view| self.document(view.doc));
            EditorContext {
                generation: self.plugins.shared.generation,
                mode: self.mode.to_string(),
                document: doc.and_then(|doc| snapshot(doc).ok()),
                view: Some(ViewSnapshot {
                    id: target.view,
                    document: target.document,
                    binding_revision: target.binding_revision,
                    selection_revision: target.selection_revision,
                    selections: Vec::new(),
                    primary: 0,
                }),
            }
        } else {
            self.plugin_global_context()
        };
        self.plugins
            .asynchronous
            .navigation
            .insert((origin.plugin.clone(), origin.sequence), context);
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
            || self.plugins.asynchronous.asset_loading.is_some()
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
        event: Event,
    ) -> Result<Arc<dyn HostServices>, ServiceError> {
        if event == Event::Shutdown {
            // Retained resource handles still own cancellation/drop cleanup.
            // A final hook may log or return Status/Error, but cannot acquire
            // new native work through the service adapter.
            return Ok(Arc::new(ShutdownServices));
        }
        let policy = self.plugins.manager.policy(plugin).ok_or_else(cancelled)?;
        let adapter: Arc<dyn HostServices> = Arc::new(EditorServices {
            owner: Arc::downgrade(&self.plugins.shared),
            callbacks: self.handlers.callbacks.clone(),
            policy: policy.clone(),
            plugin: plugin.into(),
            event,
            reads: self.plugins.asynchronous.reads.clone(),
            open_bytes: self.plugins.asynchronous.open_bytes.clone(),
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
        Ok(Arc::new(LifecycleServices {
            owner: Arc::downgrade(&self.plugins.shared),
            native: Arc::new(native.with_editor(adapter)),
        }))
    }

    fn prepare_plugin_assets(&mut self, prepared: PreparedManager, transition: AssetTransition) {
        let packages = prepared.assets();
        // No contribution means no extra worker, loader swap or syntax refresh.
        if self.plugins.assets.owners().next().is_none()
            && packages.iter().all(|package| {
                package.themes.is_empty()
                    && package.languages.is_empty()
                    && package.snippets.is_empty()
            })
        {
            self.accept_plugin_replacement(prepared, None, transition);
            return;
        }
        let input = self.plugins.assets.capture(self, packages);
        let work = self.plugins.asynchronous.reads.clone();
        self.plugins.asynchronous.asset_loading = Some(AssetLoading {
            prepared,
            transition,
            future: Box::pin(async move {
                let permit = work.acquire_owned().await.map_err(|_| cancelled())?;
                tokio::task::spawn_blocking(move || {
                    // Cancellation cannot release this permit while native
                    // query/theme/snippet compilation is still running.
                    let _permit = permit;
                    input.prepare().map_err(|error| {
                        ServiceError::new(
                            ErrorCode::InvalidRequest,
                            format!("invalid plugin contributions: {error:#}"),
                        )
                    })
                })
                .await
                .map_err(|error| ServiceError::new(ErrorCode::HostFailure, error.to_string()))?
            }),
        });
    }

    fn accept_plugin_replacement(
        &mut self,
        prepared: PreparedManager,
        assets: Option<assets::PreparedAssets>,
        transition: AssetTransition,
    ) {
        self.plugins.asynchronous.replacement = Some(Replacement {
            prepared,
            assets,
            observer: transition.observer,
            task: transition.task,
            shutdown_sent: transition.shutdown_sent,
            asset_attempts: transition.attempts,
        });
        if transition.retiring {
            return;
        }
        self.plugins.shutting_down = true;
        self.plugins.shared.quiescing.store(true, Ordering::Release);
        self.cancel_plugin_frontend();
        self.plugins
            .shared
            .accepting
            .store(false, Ordering::Release);
        // Retire the old generation only after all native contributions have
        // validated against the current resource configuration.
        self.plugins.asynchronous.calls.clear();
        self.plugins.asynchronous.call_count = 0;
        self.plugins.asynchronous.navigation.clear();
    }

    fn fail_plugin_assets(&mut self, mut error: ServiceError, transition: AssetTransition) {
        if transition.retiring {
            // Shutdown is final: preserve contributed registries for the next
            // explicit reload, but never resume an already shut-down instance.
            self.plugins.manager.retire_instances();
            self.plugins.shutting_down = false;
            self.plugins
                .shared
                .accepting
                .store(false, Ordering::Release);
            self.refresh_plugin_subscriptions();
            error
                .message
                .push_str("; prior plugin instances are shut down, reload required");
        }
        if let Some(task) = transition.task {
            task.finish(TaskOutcome::Error(error.to_string()));
        }
        self.report_plugin_error("loader", &error);
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
            let worker_succeeded = result.is_ok();
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
                worker_succeeded,
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
            for mut ready in batch {
                if let Some(context) = self
                    .plugins
                    .asynchronous
                    .navigation
                    .remove(&(ready.plugin.clone(), ready.provenance.sequence))
                {
                    ready.context = context;
                }
                let previous = self.replace_invocation_tasks(ready.observer);
                if ready.worker_succeeded
                    && let Err(error) = &ready.result
                {
                    // Deferred scoped Open preparation is host work. Its
                    // failure must correct the worker's provisional success,
                    // while an actual runtime error was counted by the actor.
                    self.plugins
                        .manager
                        .record_application(&ready.plugin, 0, Some(error));
                }
                let result = ready
                    .result
                    .map_err(anyhow::Error::from)
                    .and_then(|completed| {
                        completed.with_response(|response| {
                            let started = std::time::Instant::now();
                            let result = self.apply_plugin_response(
                                response,
                                &ready.context,
                                &ready.plugin,
                                &ready.provenance,
                                ready.event,
                                ready.opened,
                            );
                            let error = result.as_ref().err().map(|error| {
                                error
                                    .downcast_ref::<ServiceError>()
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        ServiceError::new(
                                            if error.downcast_ref::<PluginConflict>().is_some() {
                                                ErrorCode::StaleState
                                            } else {
                                                ErrorCode::InvalidRequest
                                            },
                                            error.to_string(),
                                        )
                                    })
                            });
                            self.plugins.manager.record_application(
                                &ready.plugin,
                                started.elapsed().as_micros().min(u64::MAX as u128) as u64,
                                error.as_ref(),
                            );
                            result
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
                    self.prepare_plugin_assets(
                        prepared,
                        AssetTransition {
                            observer: loading.observer,
                            task: loading.task,
                            retiring: false,
                            shutdown_sent: false,
                            attempts: 0,
                        },
                    );
                }
                Err(error) => {
                    if let Some(task) = loading.task {
                        task.finish(TaskOutcome::Error(error.to_string()));
                    }
                    self.report_plugin_error("loader", &error);
                }
            }
        }
        if let Some(loading) = self.plugins.asynchronous.asset_loading.as_mut()
            && let Poll::Ready(result) = loading.future.as_mut().poll(&mut cx)
        {
            let loading = self.plugins.asynchronous.asset_loading.take().unwrap();
            match result {
                Ok(assets) if assets.is_current(self) => self.accept_plugin_replacement(
                    loading.prepared,
                    Some(assets),
                    loading.transition,
                ),
                Ok(_) => {
                    if loading.transition.attempts < MAX_ASSET_RETRIES {
                        let mut transition = loading.transition;
                        transition.attempts += 1;
                        self.prepare_plugin_assets(loading.prepared, transition);
                        self.poll_plugin_completions();
                    } else {
                        self.fail_plugin_assets(ServiceError::new(ErrorCode::StaleState,
                            "native resources repeatedly changed during contribution preparation"),loading.transition);
                    }
                }
                Err(error) => self.fail_plugin_assets(error, loading.transition),
            }
        }
        self.advance_plugin_replacement();
        self.plugins
            .shared
            .async_pending
            .store(self.has_pending_plugin_work(), Ordering::Release);
    }

    fn advance_plugin_replacement(&mut self) {
        if self.plugins.asynchronous.advancing_replacement
            || self.plugins.asynchronous.replacement.is_none()
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
            // Dispatch polls readiness synchronously. A replacement with no
            // live old subscribers must not recursively activate twice.
            self.plugins.asynchronous.advancing_replacement = true;
            self.dispatch_plugin_event(Event::Shutdown, Value::Null);
            self.plugins.asynchronous.advancing_replacement = false;
            if !self.plugins.asynchronous.calls.is_empty() {
                return;
            }
        }
        let replacement = self.plugins.asynchronous.replacement.take().unwrap();
        // Native configuration can change while old hooks drain. Reprepare
        // against that baseline, retaining the paused replacement and never
        // delivering a second Shutdown to the old generation.
        if replacement
            .assets
            .as_ref()
            .is_some_and(|assets| !assets.is_current(self))
        {
            let mut transition = AssetTransition {
                observer: replacement.observer,
                task: replacement.task,
                retiring: true,
                shutdown_sent: true,
                attempts: replacement.asset_attempts,
            };
            if transition.attempts < MAX_ASSET_RETRIES {
                transition.attempts += 1;
                self.prepare_plugin_assets(replacement.prepared, transition);
                self.poll_plugin_completions();
            } else {
                self.fail_plugin_assets(
                    ServiceError::new(
                        ErrorCode::StaleState,
                        "native resources repeatedly changed while the previous generation drained",
                    ),
                    transition,
                );
            }
            return;
        }
        let manager = match replacement.prepared.activate() {
            Ok(manager) => manager,
            Err(error) => {
                self.fail_plugin_assets(
                    error,
                    AssetTransition {
                        observer: replacement.observer,
                        task: replacement.task,
                        retiring: true,
                        shutdown_sent: true,
                        attempts: replacement.asset_attempts,
                    },
                );
                return;
            }
        };
        self.clear_all_plugin_settings();
        if let Some(assets) = replacement.assets {
            let mut registry = std::mem::take(&mut self.plugins.assets);
            let result = assets.activate(&mut registry, self);
            self.plugins.assets = registry;
            if let Err(error) = result {
                manager.revoke();
                self.plugins
                    .asynchronous
                    .cleanup
                    .push(Box::pin(async move { manager.shutdown().await }));
                self.fail_plugin_assets(
                    ServiceError::new(ErrorCode::StaleState, error.to_string()),
                    AssetTransition {
                        observer: replacement.observer,
                        task: replacement.task,
                        retiring: true,
                        shutdown_sent: true,
                        attempts: replacement.asset_attempts,
                    },
                );
                return;
            }
        }
        let old = std::mem::replace(&mut self.plugins.manager, manager);
        old.revoke();
        self.plugins
            .asynchronous
            .cleanup
            .push(Box::pin(async move { old.shutdown().await }));
        self.plugins.asynchronous.services.clear();
        self.plugins.frontend = frontend::FrontendState::default();
        self.plugins
            .shared
            .accepting
            .store(false, Ordering::Release);
        self.plugins.shared.captures.cancel();
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
            doc.plugin_settings = Some(Arc::downgrade(&self.plugins.settings));
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
            || self.plugins.asynchronous.asset_loading.is_some()
            || self.plugins.asynchronous.replacement.is_some()
            || !self.plugins.asynchronous.cleanup.is_empty()
            || !self.plugins.shared.captures.pending.lock().is_empty()
            || !queue.pending.is_empty()
            || queue.gap.is_some()
    }

    pub(super) fn begin_plugin_shutdown(&mut self) {
        if self.plugins.stopped {
            return;
        }
        let first = !self.plugins.shutting_down;
        self.plugins.shutting_down = true;
        self.plugins.shared.quiescing.store(true, Ordering::Release);
        self.plugins.asynchronous.loading = None;
        self.plugins.asynchronous.asset_loading = None;
        self.plugins.asynchronous.replacement = None;
        if first {
            self.cancel_plugin_frontend();
        }
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
                    self.plugins.asynchronous.navigation.clear();
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
        self.plugins.asynchronous.navigation.clear();
        self.plugins.shared.captures.cancel();
        self.plugins.shared.queue.lock().pending.clear();
        self.plugins.shared.queue.lock().bytes = 0;
        if let Err(error) = self.plugins.manager.shutdown().await {
            self.report_plugin_error("shutdown", &error);
        }
        self.plugins.asynchronous.services.clear();
        self.clear_all_plugin_settings();
        let mut registry = std::mem::take(&mut self.plugins.assets);
        registry.restore(self);
        self.plugins.assets = registry;
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

fn shutdown_denied<T: Send + 'static>() -> HostFuture<T> {
    Box::pin(async {
        Err(ServiceError::new(
            ErrorCode::PermissionDenied,
            "shutdown hooks only support diagnostics and owned-resource cleanup",
        ))
    })
}

struct ShutdownServices;
impl HostServices for ShutdownServices {
    fn read_document(&self, _request: ReadRequest) -> HostFuture<String> {
        shutdown_denied()
    }
    fn notify_job_ready(&self, _job: u64) -> HostFuture<()> {
        shutdown_denied()
    }
    fn editor_request(
        &self,
        _request: plugin_api::editor::EditorRequest,
    ) -> HostFuture<plugin_api::editor::EditorReply> {
        shutdown_denied()
    }
    fn read_file(&self, _root: u32, _path: String) -> HostFuture<String> {
        shutdown_denied()
    }
    fn write_file(&self, _root: u32, _path: String, _value: String) -> HostFuture<()> {
        shutdown_denied()
    }
    fn start_job(
        &self,
        _request: plugin_api::JobRequest,
    ) -> HostFuture<Arc<dyn plugin_api::HostJob>> {
        shutdown_denied()
    }
    fn storage_read(&self, _key: String) -> HostFuture<Option<String>> {
        shutdown_denied()
    }
    fn storage_write(&self, _key: String, _value: String) -> HostFuture<()> {
        shutdown_denied()
    }
}

/// Existing accepted lifecycle invocations retain bounded reads, while close
/// or replacement prevents them from starting new native mutations or jobs.
/// Futures already admitted before quiescence keep their normal owned cleanup.
struct LifecycleServices {
    owner: Weak<Shared>,
    native: Arc<dyn HostServices>,
}

impl LifecycleServices {
    fn active<T: Send + 'static>(
        &self,
        operation: impl FnOnce(Arc<dyn HostServices>) -> HostFuture<T> + Send + 'static,
    ) -> HostFuture<T> {
        let owner = self.owner.clone();
        let native = self.native.clone();
        Box::pin(async move {
            let shared = owner.upgrade().ok_or_else(cancelled)?;
            if shared.quiescing.load(Ordering::Acquire) {
                return Err(ServiceError::new(
                    ErrorCode::PermissionDenied,
                    "plugin work is draining; new native operations are denied",
                ));
            }
            operation(native).await
        })
    }
}

impl HostServices for LifecycleServices {
    fn read_document(&self, request: ReadRequest) -> HostFuture<String> {
        self.native.read_document(request)
    }
    fn notify_job_ready(&self, job: u64) -> HostFuture<()> {
        self.active(move |native| native.notify_job_ready(job))
    }
    fn editor_request(
        &self,
        request: plugin_api::editor::EditorRequest,
    ) -> HostFuture<plugin_api::editor::EditorReply> {
        self.active(move |native| native.editor_request(request))
    }
    fn read_file(&self, root: u32, path: String) -> HostFuture<String> {
        self.native.read_file(root, path)
    }
    fn write_file(&self, root: u32, path: String, value: String) -> HostFuture<()> {
        self.active(move |native| native.write_file(root, path, value))
    }
    fn start_job(
        &self,
        request: plugin_api::JobRequest,
    ) -> HostFuture<Arc<dyn plugin_api::HostJob>> {
        self.active(move |native| native.start_job(request))
    }
    fn storage_read(&self, key: String) -> HostFuture<Option<String>> {
        // A first private read can lazily create its storage directory.
        self.active(move |native| native.storage_read(key))
    }
    fn storage_write(&self, key: String, value: String) -> HostFuture<()> {
        self.active(move |native| native.storage_write(key, value))
    }
}

struct EditorServices {
    owner: Weak<Shared>,
    callbacks: EditorCallbackSender,
    policy: Arc<::plugins::policy::AccessPolicy>,
    plugin: String,
    reads: Arc<tokio::sync::Semaphore>,
    open_bytes: Arc<tokio::sync::Semaphore>,
    source: PluginEventSource,
    event: Event,
}
impl HostServices for EditorServices {
    fn editor_request(
        &self,
        request: plugin_api::editor::EditorRequest,
    ) -> HostFuture<plugin_api::editor::EditorReply> {
        if language::handles(&request) {
            language::request(
                request,
                self.owner.clone(),
                self.callbacks.clone(),
                self.policy.clone(),
                self.reads.clone(),
            )
        } else {
            services::request(
                request,
                services::Scope {
                    owner: self.owner.clone(),
                    callbacks: self.callbacks.clone(),
                    policy: self.policy.clone(),
                    source: self.source.clone(),
                    event: self.event,
                    work: self.reads.clone(),
                    open_bytes: self.open_bytes.clone(),
                },
            )
        }
    }

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
        let event = self.event;
        Box::pin(async move {
            policy.require(Capability::EditorRead)?;
            if event == Event::Shutdown {
                return Err(ServiceError::new(
                    ErrorCode::PermissionDenied,
                    "shutdown hooks only support diagnostics",
                ));
            }
            let shared = owner.upgrade().ok_or_else(cancelled)?;
            if shared.generation != generation || shared.generation != request.generation {
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
            policy.require(Capability::EditorRead)?;
            shared.captures.enqueue(ReadCapture {
                request,
                policy: policy.clone(),
                reply: send,
            })?;
            shared.async_woken.store(true, Ordering::Release);
            PluginEventSender { owner, callbacks }.schedule_wake();
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
            Ok(value)
        })
    }
}

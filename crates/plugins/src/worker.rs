//! Bounded per-plugin actors on a dedicated executor; the editor never polls WASM.

use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, Weak,
    },
    task::{Context, Poll},
    thread,
    time::{Duration, Instant},
};

use plugin_api::{
    diagnostics::{DiagnosticEntry, DiagnosticLevel, RequestTimings, MAX_DIAGNOSTIC_ENTRIES},
    Action, CapabilitySet, ErrorCode, Event, HostServices, Request, Response, ServiceError,
};
use tokio::sync::{oneshot, Notify, Semaphore};
use tokio_util::sync::CancellationToken;
use wasmtime::{component::Component, Engine};

use crate::component::{self, Instance, MAX_COMPONENT_BYTES, MAX_MESSAGE_BYTES};

const MAX_CALLS: usize = 64;
const MAX_ACTIVE_STORES: usize = 32;
const COMMAND_RESERVE: usize = 8;
const MAX_QUEUE_BYTES: usize = 4 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_EDITOR_RESULTS: usize = 32 * 1024 * 1024;
const MAX_CACHED_BYTES: usize = 64 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 128 * 1024 * 1024;
const MAX_NATIVE_BYTES: usize = 128 * 1024 * 1024;
const MAX_CACHED_COMPONENTS: usize = 16;
const CALL_DEADLINE: Duration = Duration::from_secs(5);
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);
const EPOCH_SLICE: Duration = Duration::from_millis(2);

#[derive(Clone, Copy, Debug, Default)]
pub struct InvocationTarget {
    pub document: Option<u64>,
    pub view: Option<u64>,
    pub binding_revision: Option<u64>,
    /// Host-owned provenance; never accepted from guest metadata.
    pub call_sequence: Option<u64>,
}
impl InvocationTarget {
    pub(crate) fn from_context(context: &plugin_api::EditorContext) -> Self {
        Self {
            document: context.document.as_ref().map(|doc| doc.id),
            view: context.view.as_ref().map(|view| view.id),
            binding_revision: context.view.as_ref().map(|view| view.binding_revision),
            call_sequence: None,
        }
    }
}

pub struct Completion {
    receiver: oneshot::Receiver<Result<CompletedResponse, ServiceError>>,
    _admission: tokio::sync::OwnedSemaphorePermit,
    cancel: CancellationToken,
}
impl Future for Completion {
    type Output = Result<CompletedResponse, ServiceError>;
    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.receiver).poll(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(_)) => Poll::Ready(Err(error(
                ErrorCode::HostFailure,
                "plugin worker stopped before completing its call",
            ))),
        }
    }
}
impl Drop for Completion {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn error(code: ErrorCode, message: &str) -> ServiceError {
    ServiceError::new(code, message)
}
fn cancelled() -> ServiceError {
    error(ErrorCode::Cancelled, "plugin work was cancelled")
}
fn exhausted(message: &str) -> ServiceError {
    error(ErrorCode::ResourceExhausted, message)
}
fn deadline_exceeded() -> ServiceError {
    error(
        ErrorCode::DeadlineExceeded,
        "plugin preparation or execution deadline exceeded",
    )
}

struct EpochTicker {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl EpochTicker {
    fn new(engine: &Engine) -> Result<Self, ServiceError> {
        let weak = engine.weak();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let thread = thread::Builder::new()
            .name("plugin-epochs".into())
            .spawn(move || {
                while !signal.load(Ordering::Acquire) {
                    thread::sleep(EPOCH_SLICE);
                    let Some(engine) = weak.upgrade() else { break };
                    engine.increment_epoch();
                }
            })
            .map_err(|cause| {
                ServiceError::new(
                    ErrorCode::HostFailure,
                    format!("spawning plugin epoch ticker: {cause}"),
                )
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
struct Cache {
    entries: VecDeque<(Arc<[u8]>, Arc<Compiled>, usize)>,
    bytes: usize,
}
struct CodeReservation {
    counter: Arc<AtomicUsize>,
    bytes: usize,
}
impl Drop for CodeReservation {
    fn drop(&mut self) {
        self.counter.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
pub(crate) struct Compiled {
    pub component: Component,
    _reservation: CodeReservation,
}
#[derive(Default)]
struct Actors {
    count: AtomicUsize,
    drained: Notify,
}
struct PoolInner {
    handle: tokio::runtime::Handle,
    stop: CancellationToken,
    actors: Arc<Actors>,
    engine: Mutex<Option<Engine>>,
    ticker: Mutex<Option<EpochTicker>>,
    compile: Arc<Semaphore>,
    execute: Arc<Semaphore>,
    cache: Mutex<Cache>,
    memory: Arc<AtomicUsize>,
    code_bytes: Arc<AtomicUsize>,
    native_bytes: Arc<AtomicUsize>,
    result_bytes: Arc<AtomicUsize>,
}
impl Drop for PoolInner {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// One executor per owning editor. Engine creation and compilation are lazy.
#[derive(Clone)]
pub struct WorkerPool {
    inner: Arc<PoolInner>,
}
impl WorkerPool {
    #[cfg(test)]
    pub(crate) fn same_executor(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
    pub fn new() -> Result<Self, ServiceError> {
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let actors = Arc::new(Actors::default());
        let drain = actors.clone();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        thread::Builder::new()
            .name("plugin-executor".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .max_blocking_threads(1)
                    .thread_name("plugin-worker")
                    .event_interval(1)
                    .global_queue_interval(1)
                    .enable_all()
                    .build();
                match runtime {
                    Err(cause) => {
                        let _ = send.send(Err(ServiceError::new(
                            ErrorCode::HostFailure,
                            format!("creating plugin executor: {cause}"),
                        )));
                    }
                    Ok(runtime) => {
                        if send.send(Ok(runtime.handle().clone())).is_err() {
                            return;
                        }
                        runtime.block_on(async {
                            stopped.cancelled().await;
                            let deadline = Instant::now() + SHUTDOWN_DEADLINE;
                            while drain.count.load(Ordering::Acquire) != 0 {
                                let notified = drain.drained.notified();
                                if drain.count.load(Ordering::Acquire) == 0 {
                                    break;
                                }
                                if tokio::time::timeout_at(deadline.into(), notified)
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        });
                        runtime.shutdown_timeout(Duration::from_secs(1));
                    }
                }
            })
            .map_err(|cause| {
                ServiceError::new(
                    ErrorCode::HostFailure,
                    format!("spawning plugin executor: {cause}"),
                )
            })?;
        let handle = receive.recv().map_err(|_| {
            error(
                ErrorCode::HostFailure,
                "plugin executor failed during startup",
            )
        })??;
        Ok(Self {
            inner: Arc::new(PoolInner {
                handle,
                stop,
                actors,
                engine: Mutex::new(None),
                ticker: Mutex::new(None),
                compile: Arc::new(Semaphore::new(1)),
                execute: Arc::new(Semaphore::new(MAX_ACTIVE_STORES)),
                cache: Mutex::new(Cache::default()),
                memory: Arc::new(AtomicUsize::new(0)),
                code_bytes: Arc::new(AtomicUsize::new(0)),
                native_bytes: Arc::new(AtomicUsize::new(0)),
                result_bytes: Arc::new(AtomicUsize::new(0)),
            }),
        })
    }
    pub fn spawn(
        &self,
        bytes: Arc<[u8]>,
        generation: u64,
        declared: CapabilitySet,
        granted: CapabilitySet,
    ) -> Result<PluginActor, ServiceError> {
        if bytes.len() > MAX_COMPONENT_BYTES {
            return Err(exhausted("plugin component exceeds its source byte limit"));
        }
        self.inner
            .code_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes.len())
                    .filter(|total| *total <= MAX_SOURCE_BYTES)
            })
            .map_err(|_| exhausted("editor plugin source budget exceeded"))?;
        let job_controls = Arc::new(component::JobControls::default());
        let mailbox = Arc::new(Mutex::new(Mailbox::default()));
        let wake = Arc::new(Notify::new());
        let cancel = self.inner.stop.child_token();
        let result_bytes = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicBool::new(true));
        let current = Arc::new(Mutex::new(None));
        let done = Arc::new(ActorDone::default());
        let diagnostics = Arc::new(Mutex::new(ActorDiagnostics::default()));
        let actor = PluginActor {
            inner: Arc::new(ActorOwner {
                _pool: self.clone(),
                job_controls: job_controls.clone(),
                mailbox: mailbox.clone(),
                wake: wake.clone(),
                cancel: cancel.clone(),
                active: active.clone(),
                current: current.clone(),
                done: done.clone(),
                diagnostics: diagnostics.clone(),
                admission: Arc::new(Semaphore::new(MAX_CALLS)),
                _result_bytes: result_bytes.clone(),
            }),
        };
        self.inner.actors.count.fetch_add(1, Ordering::AcqRel);
        self.inner.handle.spawn(actor_loop(
            Arc::downgrade(&self.inner),
            self.inner.actors.clone(),
            SourceReservation {
                counter: self.inner.code_bytes.clone(),
                bytes: bytes.len(),
            },
            bytes,
            generation,
            declared,
            granted,
            None,
            result_bytes,
            job_controls,
            mailbox,
            wake,
            cancel,
            active,
            current,
            done,
            diagnostics,
        ));
        Ok(actor)
    }
    /// Compile, link-check and instantiate a replacement without running Init or
    /// attaching editor services. The owning editor activates only after every
    /// package in its replacement set has prepared successfully.
    pub fn prepare(
        &self,
        bytes: Arc<[u8]>,
        generation: u64,
        declared: CapabilitySet,
        granted: CapabilitySet,
    ) -> Result<Preparation, ServiceError> {
        if bytes.len() > MAX_COMPONENT_BYTES {
            return Err(exhausted("plugin component exceeds its source byte limit"));
        }
        self.inner
            .code_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes.len())
                    .filter(|total| *total <= MAX_SOURCE_BYTES)
            })
            .map_err(|_| exhausted("editor plugin source budget exceeded"))?;
        let reservation = SourceReservation {
            counter: self.inner.code_bytes.clone(),
            bytes: bytes.len(),
        };
        let cancel = self.inner.stop.child_token();
        let stopping = cancel.clone();
        let (send, receiver) = oneshot::channel();
        let pool = self.clone();
        self.inner.actors.count.fetch_add(1, Ordering::AcqRel);
        self.inner.handle.spawn(async move {
            let _activity = Activity(pool.inner.actors.clone());
            let result = async {
                let deadline = Instant::now() + CALL_DEADLINE;
                let _permit = tokio::select! { _ = stopping.cancelled() => return Err(cancelled()), _ = tokio::time::sleep_until(deadline.into()) => return Err(deadline_exceeded()), permit = pool.inner.execute.clone().acquire_owned() => permit.map_err(|_| cancelled())? };
                let (engine, component) = compiled(&pool.inner, bytes.clone(), &stopping, deadline).await?;
                let instance = Instance::new(&engine, &component, component::InstanceOptions { generation, declared: declared.clone(), granted: granted.clone(), cancel: stopping, deadline, shared_memory: pool.inner.memory.clone() }).await?;
                Ok(PreparedPlugin { pool, bytes, generation, declared, granted, instance: Some(instance), reservation: Some(reservation) })
            }.await;
            let _ = send.send(result);
        });
        Ok(Preparation { receiver, cancel })
    }

    /// Move an already prepared store into its serialized actor. This operation
    /// neither compiles nor invokes a guest and is safe on an editor callback.
    pub fn activate(&self, mut prepared: PreparedPlugin) -> Result<PluginActor, ServiceError> {
        if !Arc::ptr_eq(&self.inner, &prepared.pool.inner) || self.inner.stop.is_cancelled() {
            return Err(cancelled());
        }
        let job_controls = Arc::new(component::JobControls::default());
        let mailbox = Arc::new(Mutex::new(Mailbox::default()));
        let wake = Arc::new(Notify::new());
        let cancel = self.inner.stop.child_token();
        let result_bytes = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicBool::new(true));
        let current = Arc::new(Mutex::new(None));
        let done = Arc::new(ActorDone::default());
        let diagnostics = Arc::new(Mutex::new(ActorDiagnostics::default()));
        let actor = PluginActor {
            inner: Arc::new(ActorOwner {
                _pool: self.clone(),
                job_controls: job_controls.clone(),
                mailbox: mailbox.clone(),
                wake: wake.clone(),
                cancel: cancel.clone(),
                active: active.clone(),
                current: current.clone(),
                done: done.clone(),
                diagnostics: diagnostics.clone(),
                admission: Arc::new(Semaphore::new(MAX_CALLS)),
                _result_bytes: result_bytes.clone(),
            }),
        };
        let instance = prepared.instance.take();
        let reservation = prepared.reservation.take().unwrap();
        self.inner.actors.count.fetch_add(1, Ordering::AcqRel);
        self.inner.handle.spawn(actor_loop(
            Arc::downgrade(&self.inner),
            self.inner.actors.clone(),
            reservation,
            prepared.bytes.clone(),
            prepared.generation,
            prepared.declared.clone(),
            prepared.granted.clone(),
            instance,
            result_bytes,
            job_controls,
            mailbox,
            wake,
            cancel,
            active,
            current,
            done,
            diagnostics,
        ));
        Ok(actor)
    }

    /// Signal revocation before waiting; cancellation does not need mailbox space.
    pub async fn shutdown(&self) -> Result<(), ServiceError> {
        self.inner.stop.cancel();
        let deadline = Instant::now() + SHUTDOWN_DEADLINE;
        loop {
            let notified = self.inner.actors.drained.notified();
            if self.inner.actors.count.load(Ordering::Acquire) == 0 {
                return Ok(());
            }
            tokio::time::timeout_at(deadline.into(), notified)
                .await
                .map_err(|_| {
                    error(
                        ErrorCode::DeadlineExceeded,
                        "plugin cleanup exceeded its shutdown deadline",
                    )
                })?;
        }
    }
}

/// Retains response-byte reservations through the owning editor callback.
/// Use `with_response` at application; extracting an unaccounted response is not
/// provided. Dropping a cancelled/unconsumed callback releases its reservation.
pub struct CompletedResponse {
    response: Response,
    _reservation: ResultReservation,
}
impl std::fmt::Debug for CompletedResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.response.fmt(f)
    }
}
impl std::ops::Deref for CompletedResponse {
    type Target = Response;
    fn deref(&self) -> &Response {
        &self.response
    }
}
impl CompletedResponse {
    pub fn with_response<T>(self, apply: impl FnOnce(Response) -> T) -> T {
        let Self {
            response,
            _reservation,
        } = self;
        let result = apply(response);
        drop(_reservation);
        result
    }
}
struct ResultReservation {
    actor: Arc<AtomicUsize>,
    editor: Arc<AtomicUsize>,
    bytes: usize,
}
impl Drop for ResultReservation {
    fn drop(&mut self) {
        self.actor.fetch_sub(self.bytes, Ordering::AcqRel);
        self.editor.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
fn deliver(
    response: Response,
    actor: Arc<AtomicUsize>,
    editor: Arc<AtomicUsize>,
) -> Result<CompletedResponse, ServiceError> {
    let bytes = response_bytes(&response);
    actor
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current
                .checked_add(bytes)
                .filter(|total| *total <= MAX_MESSAGE_BYTES)
        })
        .map_err(|_| exhausted("plugin retained response bytes exceeded"))?;
    if editor
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current
                .checked_add(bytes)
                .filter(|total| *total <= MAX_EDITOR_RESULTS)
        })
        .is_err()
    {
        actor.fetch_sub(bytes, Ordering::AcqRel);
        return Err(exhausted("editor retained plugin response bytes exceeded"));
    }
    Ok(CompletedResponse {
        response,
        _reservation: ResultReservation {
            actor,
            editor,
            bytes,
        },
    })
}
fn response_bytes(response: &Response) -> usize {
    let mut bytes = std::mem::size_of::<Response>()
        + response.actions.capacity() * std::mem::size_of::<Action>();
    bytes += response.error.as_ref().map_or(0, String::capacity);
    for action in &response.actions {
        bytes = bytes.saturating_add(match action {
            Action::Status { message } | Action::Error { message } => message.capacity(),
            Action::Open { path } => path.capacity(),
            Action::ShowUi { kind, .. } => match kind {
                plugin_api::ui::UiKind::Prompt { title, initial } => {
                    title.capacity() + initial.capacity()
                }
                plugin_api::ui::UiKind::NextKey { title, .. } => title.capacity(),
                plugin_api::ui::UiKind::Picker { title, rows } => {
                    title.capacity()
                        + rows.capacity() * std::mem::size_of::<plugin_api::ui::UiRow>()
                        + rows
                            .iter()
                            .map(|row| {
                                row.id.capacity()
                                    + row.label.capacity()
                                    + row.description.capacity()
                                    + row.preview.as_ref().map_or(0, String::capacity)
                            })
                            .sum::<usize>()
                }
            },
            Action::InvokeBuiltin { commands, .. } => {
                commands.capacity() * std::mem::size_of::<plugin_api::ui::BuiltinInvocation>()
            }
            Action::UpdateKeymap { bindings, .. } => {
                bindings.capacity() * std::mem::size_of::<plugin_api::ui::PluginKeybinding>()
                    + bindings
                        .iter()
                        .map(|binding| {
                            binding.command.capacity()
                                + binding.keys.capacity() * std::mem::size_of::<String>()
                                + binding.keys.iter().map(String::capacity).sum::<usize>()
                        })
                        .sum::<usize>()
            }
            Action::RequestState { .. } => 0,
            Action::Edit { edits, .. } => {
                edits.capacity() * std::mem::size_of::<plugin_api::TextEdit>()
                    + edits.iter().map(|edit| edit.text.capacity()).sum::<usize>()
            }
            Action::SetSelection { ranges, .. } => {
                ranges.capacity() * std::mem::size_of::<plugin_api::SelectionRange>()
            }
        });
    }
    bytes
}

struct SourceReservation {
    counter: Arc<AtomicUsize>,
    bytes: usize,
}
impl Drop for SourceReservation {
    fn drop(&mut self) {
        self.counter.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
struct Activity(Arc<Actors>);
impl Drop for Activity {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::AcqRel);
        self.0.drained.notify_waiters();
    }
}

pub struct Preparation {
    receiver: oneshot::Receiver<Result<PreparedPlugin, ServiceError>>,
    cancel: CancellationToken,
}
impl Future for Preparation {
    type Output = Result<PreparedPlugin, ServiceError>;
    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.receiver).poll(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(_)) => Poll::Ready(Err(error(
                ErrorCode::HostFailure,
                "plugin preparation worker stopped",
            ))),
        }
    }
}
impl Drop for Preparation {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub struct PreparedPlugin {
    pool: WorkerPool,
    bytes: Arc<[u8]>,
    generation: u64,
    declared: CapabilitySet,
    granted: CapabilitySet,
    instance: Option<Instance>,
    reservation: Option<SourceReservation>,
}
impl Drop for PreparedPlugin {
    fn drop(&mut self) {
        if let Some(mut instance) = self.instance.take() {
            self.pool.inner.handle.spawn(async move {
                instance.cleanup().await;
            });
        }
    }
}

struct Envelope {
    submitted: Instant,
    config_json: Option<Arc<str>>,
    target: InvocationTarget,
    cancel: CancellationToken,
    request: Request,
    services: Arc<dyn HostServices>,
    bytes: usize,
    result: oneshot::Sender<Result<CompletedResponse, ServiceError>>,
}
#[derive(Default)]
struct Mailbox {
    commands: VecDeque<Envelope>,
    notifications: VecDeque<Envelope>,
    bytes: usize,
}

#[derive(Default)]
struct ActorDiagnostics {
    timings: RequestTimings,
    entries: VecDeque<DiagnosticEntry>,
    sequence: u64,
}
impl ActorDiagnostics {
    fn log(&mut self, level: DiagnosticLevel, message: &str) {
        self.sequence = self.sequence.saturating_add(1);
        if self.entries.len() == MAX_DIAGNOSTIC_ENTRIES {
            self.entries.pop_front();
        }
        self.entries
            .push_back(DiagnosticEntry::bounded(self.sequence, level, message));
    }
    fn failed(&mut self, error: &ServiceError) {
        if error.code == ErrorCode::Cancelled {
            self.timings.cancelled = self.timings.cancelled.saturating_add(1);
        } else {
            self.timings.failed = self.timings.failed.saturating_add(1);
        }
        self.log(DiagnosticLevel::Error, &error.message);
    }
    fn record(
        &mut self,
        queued: Duration,
        execution: Duration,
        result: Result<&Response, &ServiceError>,
    ) {
        let micros = |duration: Duration| u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
        self.timings.queue_last_us = micros(queued);
        self.timings.queue_max_us = self.timings.queue_max_us.max(self.timings.queue_last_us);
        self.timings.execution_last_us = micros(execution);
        self.timings.execution_max_us = self
            .timings
            .execution_max_us
            .max(self.timings.execution_last_us);
        match result {
            Ok(response) => {
                self.timings.completed = self.timings.completed.saturating_add(1);
                if let Some(message) = &response.error {
                    self.log(DiagnosticLevel::Error, message);
                }
                for action in &response.actions {
                    match action {
                        Action::Status { message } => self.log(DiagnosticLevel::Info, message),
                        Action::Error { message } => self.log(DiagnosticLevel::Error, message),
                        _ => (),
                    }
                }
            }
            Err(error) => self.failed(error),
        }
    }
}
impl Mailbox {
    fn len(&self) -> usize {
        self.commands.len() + self.notifications.len()
    }
    fn pop(&mut self) -> Option<Envelope> {
        // Initialization is ordered before commands; later commands have reserved
        // admission and execution priority over ordinary event notifications.
        let envelope = if let Some(index) = self
            .notifications
            .iter()
            .position(|call| call.request.event == Event::Init)
        {
            self.notifications.remove(index)
        } else {
            self.commands
                .pop_front()
                .or_else(|| self.notifications.pop_front())
        }?;
        self.bytes -= envelope.bytes;
        Some(envelope)
    }
    fn fail(&mut self, cause: ServiceError, diagnostics: &Mutex<ActorDiagnostics>) {
        for envelope in self.commands.drain(..).chain(self.notifications.drain(..)) {
            diagnostics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .record(envelope.submitted.elapsed(), Duration::ZERO, Err(&cause));
            let _ = envelope.result.send(Err(cause.clone()));
        }
        self.bytes = 0;
    }
}
#[derive(Default)]
struct ActorDone {
    finished: AtomicBool,
    wake: Notify,
}
impl ActorDone {
    fn finish(&self) {
        self.finished.store(true, Ordering::Release);
        self.wake.notify_waiters();
    }
}

struct ActiveCall {
    document: Option<u64>,
    view: Option<u64>,
    call_sequence: Option<u64>,
    cancel: CancellationToken,
}
struct ActorOwner {
    job_controls: Arc<component::JobControls>,
    _pool: WorkerPool,
    mailbox: Arc<Mutex<Mailbox>>,
    wake: Arc<Notify>,
    cancel: CancellationToken,
    active: Arc<AtomicBool>,
    current: Arc<Mutex<Option<ActiveCall>>>,
    done: Arc<ActorDone>,
    diagnostics: Arc<Mutex<ActorDiagnostics>>,
    admission: Arc<Semaphore>,
    _result_bytes: Arc<AtomicUsize>,
}
impl Drop for ActorOwner {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.wake.notify_one();
    }
}

/// Cloneable submission/control handle. Its store is owned by exactly one actor.
#[derive(Clone)]
pub struct PluginActor {
    inner: Arc<ActorOwner>,
}
impl PluginActor {
    /// Bounded owned snapshots; no guest execution or filesystem work.
    pub fn diagnostics(&self) -> (usize, RequestTimings, Vec<DiagnosticEntry>) {
        let queued = self
            .inner
            .mailbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        let diagnostics = self
            .inner
            .diagnostics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            queued,
            diagnostics.timings.clone(),
            diagnostics.entries.iter().cloned().collect(),
        )
    }
    /// The host records this only for a successfully returned guest response.
    /// A rejected application changes the provisional successful outcome.
    pub fn record_application(&self, duration_us: u64, error: Option<&ServiceError>) {
        let mut diagnostics = self
            .inner
            .diagnostics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        diagnostics.timings.apply_last_us = duration_us;
        diagnostics.timings.apply_max_us = diagnostics.timings.apply_max_us.max(duration_us);
        if let Some(error) = error {
            diagnostics.timings.completed = diagnostics.timings.completed.saturating_sub(1);
            diagnostics.failed(error);
        }
    }
    pub fn is_active(&self) -> bool {
        self.inner.active.load(Ordering::Acquire) && !self.inner.cancel.is_cancelled()
    }
    pub fn is_busy(&self) -> bool {
        let mailbox = self
            .inner
            .mailbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        !mailbox.commands.is_empty()
            || !mailbox.notifications.is_empty()
            || self
                .inner
                .current
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some()
    }
    pub fn invoke(
        &self,
        request: Request,
        services: Arc<dyn HostServices>,
    ) -> Result<Completion, ServiceError> {
        let target = InvocationTarget::from_context(&request.editor);
        self.invoke_with_target(request, services, target)
    }
    pub fn invoke_with_target(
        &self,
        request: Request,
        services: Arc<dyn HostServices>,
        target: InvocationTarget,
    ) -> Result<Completion, ServiceError> {
        self.invoke_configured(request, services, target, None)
    }
    pub(crate) fn invoke_configured(
        &self,
        request: Request,
        services: Arc<dyn HostServices>,
        target: InvocationTarget,
        config_json: Option<Arc<str>>,
    ) -> Result<Completion, ServiceError> {
        if !self.is_active() {
            return Err(error(
                ErrorCode::GuestTrap,
                "plugin is disabled; reload it to resume",
            ));
        }
        let admission = self
            .inner
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| exhausted("plugin has too many outstanding calls or results"))?;
        let bytes = request_bytes(&request)?
            .saturating_add(config_json.as_ref().map_or(0, |config| config.len()));
        if bytes > MAX_REQUEST_BYTES {
            return Err(exhausted(
                "plugin configured request exceeds its byte limit",
            ));
        }
        let cancel = self.inner.cancel.child_token();
        let (send, receive) = oneshot::channel();
        let command = request.event == Event::Command;
        let mut mailbox = self
            .inner
            .mailbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Final draining and this admission share the mailbox lock. A producer
        // must not enqueue behind the final drain after the actor has stopped.
        if !self.is_active() {
            return Err(cancelled());
        }
        if mailbox.bytes.saturating_add(bytes) > MAX_QUEUE_BYTES
            || mailbox.len() >= MAX_CALLS
            || (!command && mailbox.len() >= MAX_CALLS - COMMAND_RESERVE)
        {
            return Err(exhausted(
                "plugin mailbox is full; request current state to recover",
            ));
        }
        mailbox.bytes += bytes;
        let envelope = Envelope {
            submitted: Instant::now(),
            config_json,
            target,
            cancel: cancel.clone(),
            request,
            services,
            bytes,
            result: send,
        };
        if command {
            mailbox.commands.push_back(envelope);
        } else {
            mailbox.notifications.push_back(envelope);
        }
        drop(mailbox);
        self.inner.wake.notify_one();
        Ok(Completion {
            receiver: receive,
            _admission: admission,
            cancel,
        })
    }
    /// Cancel obsolete calls without executing a guest or awaiting native work.
    pub fn job_target(&self, job: u64) -> Option<InvocationTarget> {
        self.inner.job_controls.target(job)
    }
    pub fn cancel_target(&self, document: Option<u64>, view: Option<u64>) {
        self.cancel_target_except(document, view, None);
    }
    pub fn cancel_target_except(
        &self,
        document: Option<u64>,
        view: Option<u64>,
        call_sequence: Option<u64>,
    ) {
        self.inner.job_controls.cancel_target(document, view);
        let matches = |target: InvocationTarget| {
            document.is_some_and(|id| target.document == Some(id))
                || view.is_some_and(|id| target.view == Some(id))
        };
        // Match the actor's dequeue/publication lock order so close cannot miss a
        // request between its mailbox and active-call ownership.
        let mut mailbox = self
            .inner
            .mailbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(active) = &*self
            .inner
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            && (document.is_some_and(|id| active.document == Some(id))
                || view.is_some_and(|id| active.view == Some(id)))
            && !call_sequence.is_some_and(|sequence| active.call_sequence == Some(sequence))
        {
            active.cancel.cancel();
        }
        let mut removed = 0;
        let Mailbox {
            commands,
            notifications,
            ..
        } = &mut *mailbox;
        for queue in [commands, notifications] {
            let mut kept = VecDeque::new();
            for envelope in queue.drain(..) {
                if matches(envelope.target) {
                    removed += envelope.bytes;
                    self.inner
                        .diagnostics
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .record(
                            envelope.submitted.elapsed(),
                            Duration::ZERO,
                            Err(&cancelled()),
                        );
                    let _ = envelope.result.send(Err(cancelled()));
                } else {
                    kept.push_back(envelope);
                }
            }
            *queue = kept;
        }
        mailbox.bytes -= removed;
    }
    pub fn revoke(&self) {
        self.inner.cancel.cancel();
        self.inner.wake.notify_one();
    }
    pub async fn shutdown(&self) -> Result<(), ServiceError> {
        self.revoke();
        let deadline = Instant::now() + SHUTDOWN_DEADLINE;
        loop {
            let notified = self.inner.done.wake.notified();
            if self.inner.done.finished.load(Ordering::Acquire) {
                break;
            }
            tokio::time::timeout_at(deadline.into(), notified)
                .await
                .map_err(|_| {
                    error(
                        ErrorCode::DeadlineExceeded,
                        "plugin cleanup exceeded its deadline",
                    )
                })?;
        }
        Ok(())
    }
}

async fn compiled(
    pool: &Arc<PoolInner>,
    bytes: Arc<[u8]>,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<(Engine, Arc<Compiled>), ServiceError> {
    let permit = tokio::select! { _ = cancel.cancelled() => return Err(cancelled()), _ = tokio::time::sleep_until(deadline.into()) => return Err(deadline_exceeded()), permit = pool.compile.clone().acquire_owned() => permit.map_err(|_| cancelled())? };
    let engine = {
        let mut current = pool
            .engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if current.is_none() {
            let engine = component::engine()?;
            *pool
                .ticker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(EpochTicker::new(&engine)?);
            *current = Some(engine);
        }
        current.as_ref().unwrap().clone()
    };
    {
        let mut cache = pool
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(index) = cache
            .entries
            .iter()
            .position(|(source, _, _)| source.as_ref() == bytes.as_ref())
        {
            let entry = cache.entries.remove(index).unwrap();
            let component = entry.1.clone();
            cache.entries.push_back(entry);
            return Ok((engine, component));
        }
    }
    let compiler = engine.clone();
    let source = bytes.clone();
    let native_bytes = pool.native_bytes.clone();
    let task = pool.handle.spawn_blocking(move || {
        let _permit = permit;
        crate::complexity::check(&source)?;
        let component = Component::from_binary(&compiler, &source).map_err(|cause| {
            ServiceError::new(
                ErrorCode::UnsupportedInterface,
                format!("compiling plugin component: {cause:#}"),
            )
        })?;
        // Serialize our own artifact solely to measure retained native code. No
        // package-provided native artifact is accepted or deserialized.
        let weight = source.len().saturating_add(
            component
                .serialize()
                .map_err(|cause| {
                    ServiceError::new(
                        ErrorCode::HostFailure,
                        format!("measuring plugin native code: {cause}"),
                    )
                })?
                .len(),
        );
        if weight > MAX_CACHED_BYTES {
            return Err(exhausted(
                "compiled plugin exceeds its native code byte limit",
            ));
        }
        native_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(weight)
                    .filter(|total| *total <= MAX_NATIVE_BYTES)
            })
            .map_err(|_| exhausted("editor active plugin native code budget exceeded"))?;
        Ok((
            Arc::new(Compiled {
                component,
                _reservation: CodeReservation {
                    counter: native_bytes,
                    bytes: weight,
                },
            }),
            weight,
        ))
    });
    let (component, weight) = tokio::select! { _ = cancel.cancelled() => return Err(cancelled()), _ = tokio::time::sleep_until(deadline.into()) => return Err(deadline_exceeded()), result = task => result.map_err(|_| error(ErrorCode::HostFailure, "plugin compiler worker failed"))?? };
    let mut cache = pool
        .cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while cache.entries.len() >= MAX_CACHED_COMPONENTS
        || cache.bytes.saturating_add(weight) > MAX_CACHED_BYTES
    {
        let Some((_, _, weight)) = cache.entries.pop_front() else {
            break;
        };
        cache.bytes -= weight;
    }
    cache.bytes += weight;
    cache.entries.push_back((bytes, component.clone(), weight));
    Ok((engine, component))
}

#[allow(clippy::too_many_arguments)]
async fn actor_loop(
    pool: Weak<PoolInner>,
    actors: Arc<Actors>,
    reservation: SourceReservation,
    bytes: Arc<[u8]>,
    generation: u64,
    declared: CapabilitySet,
    granted: CapabilitySet,
    mut instance: Option<Instance>,
    result_bytes: Arc<AtomicUsize>,
    job_controls: Arc<component::JobControls>,
    mailbox: Arc<Mutex<Mailbox>>,
    wake: Arc<Notify>,
    cancel: CancellationToken,
    active: Arc<AtomicBool>,
    current: Arc<Mutex<Option<ActiveCall>>>,
    done: Arc<ActorDone>,
    diagnostics: Arc<Mutex<ActorDiagnostics>>,
) {
    let _activity = Activity(actors);
    let _reservation = reservation;
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let notified = wake.notified();
        let envelope = {
            let mut mailbox = mailbox
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let envelope = mailbox.pop();
            if let Some(envelope) = &envelope {
                *current
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ActiveCall {
                    document: envelope.target.document,
                    view: envelope.target.view,
                    call_sequence: envelope.target.call_sequence,
                    cancel: envelope.cancel.clone(),
                });
            }
            envelope
        };
        let Some(envelope) = envelope else {
            tokio::select! { _ = cancel.cancelled() => break, _ = notified => continue };
        };
        let queued = envelope.submitted.elapsed();
        let initializing = envelope.request.event == Event::Init;
        if envelope.result.is_closed() {
            diagnostics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .record(queued, Duration::ZERO, Err(&cancelled()));
            *current
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            if initializing {
                active.store(false, Ordering::Release);
                break;
            }
            continue;
        }
        if envelope.request.editor.generation != generation {
            *current
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            let cause = error(
                ErrorCode::StaleState,
                "plugin request belongs to an old generation",
            );
            diagnostics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .record(queued, Duration::ZERO, Err(&cause));
            let _ = envelope.result.send(Err(cause));
            if initializing {
                active.store(false, Ordering::Release);
                break;
            }
            continue;
        }
        let invocation_cancel = envelope.cancel;
        let deadline = Instant::now() + CALL_DEADLINE;
        let started = Instant::now();
        let result = async {
            let pool = pool.upgrade().ok_or_else(cancelled)?;
            let permit = tokio::select! { _ = invocation_cancel.cancelled() => return Err(cancelled()), _ = tokio::time::sleep_until(deadline.into()) => return Err(deadline_exceeded()), permit = pool.execute.clone().acquire_owned() => permit.map_err(|_| cancelled())? };
            if instance.is_none() {
                let (engine, component) = compiled(&pool, bytes.clone(), &invocation_cancel, deadline).await?;
                instance = Some(Instance::new(&engine, &component, component::InstanceOptions { generation, declared: declared.clone(), granted: granted.clone(), cancel: invocation_cancel.clone(), deadline, shared_memory: pool.memory.clone() }).await?);
            }
            let _permit = permit;
            match instance.as_mut().unwrap().call(component::Invocation { request: envelope.request, services: envelope.services, cancel: invocation_cancel, owner_cancel: cancel.clone(), deadline, target: envelope.target, job_controls: job_controls.clone(), config_json: envelope.config_json }).await {
                Ok(response) => deliver(response, result_bytes.clone(), pool.result_bytes.clone()),
                Err(failure) => { if failure.discard { instance = None; active.store(false, Ordering::Release); } Err(failure.error) }
            }
        }.await;
        *current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        if instance.is_none()
            && result.as_ref().is_err_and(|error| {
                matches!(
                    error.code,
                    ErrorCode::GuestTrap | ErrorCode::UnsupportedInterface
                )
            })
        {
            active.store(false, Ordering::Release);
        }
        if initializing && result.is_err() {
            active.store(false, Ordering::Release);
        }
        diagnostics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .record(
                queued,
                started.elapsed(),
                result.as_ref().map(|response| &**response),
            );
        let returned_response = result.is_ok();
        if envelope.result.send(result).is_err() && returned_response {
            let mut diagnostics = diagnostics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            diagnostics.timings.completed = diagnostics.timings.completed.saturating_sub(1);
            diagnostics.failed(&cancelled());
        }
        if !active.load(Ordering::Acquire) {
            break;
        }
    }
    if let Some(mut instance) = instance.take() {
        instance.cleanup().await;
        drop(instance);
    }
    active.store(false, Ordering::Release);
    mailbox
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .fail(cancelled(), &diagnostics);
    done.finish();
}

fn request_bytes(request: &Request) -> Result<usize, ServiceError> {
    // The neutral context contains metadata, not document text. This bounded
    // writer limits host-provided arguments/data without an unbounded Vec.
    let bytes = component::bounded_json(request, MAX_REQUEST_BYTES)?.len();
    if request.args.len() > 128
        || request.args.iter().any(|arg| arg.len() > 4096)
        || request.editor.view.as_ref().is_some_and(|view| {
            view.selections.len() > 4096 || view.primary >= view.selections.len()
        })
    {
        return Err(exhausted(
            "plugin request arguments or selection summary exceed their limit",
        ));
    }
    if bytes > MAX_MESSAGE_BYTES {
        return Err(exhausted("plugin request exceeds its byte limit"));
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;

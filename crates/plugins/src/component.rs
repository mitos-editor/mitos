//! Typed component execution. All guest effects are staged until export completion.

use std::{
    collections::BTreeMap,
    io::Write,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

use plugin_api::{
    Action, Capability, CapabilitySet, ErrorCode, Event, HostJob, HostServices, JobRequest,
    ReadRequest, Request as ApiRequest, Response, SelectionRange, ServiceError, StateQuery,
    TextEdit,
};
use tokio_util::sync::CancellationToken;
use wasmtime::component::{Linker, Resource, ResourceTable};
use wasmtime::{Config, Engine, ResourceLimiter, Store, UpdateDeadline};

pub(crate) const MAX_COMPONENT_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_TOTAL_MEMORY_BYTES: usize = 256 * 1024 * 1024;
const MAX_ACTIONS: usize = 256;
const MAX_ENTRIES: usize = 4096;
const MAX_HANDLES: usize = 64;
const MAX_JOBS: usize = 16;
const MAX_ARGUMENTS: usize = 128;
const MAX_KEY_BYTES: usize = 512;
const MAX_ERROR_BYTES: usize = 4096;

plugin_api::generate_bindings!(wasmtime::component::bindgen,
    world: "plugin",
    imports: { default: async | trappable },
    exports: { default: async },
    with: {
        "mitos:plugin/host@0.1.0.effects": EffectsHandle,
        "mitos:plugin/host@0.1.0.edit-group": EditHandle,
        "mitos:plugin/host@0.1.0.selection-group": SelectionHandle,
        "mitos:plugin/host@0.1.0.job": JobHandle,
        "mitos:plugin/host@0.1.0.process-request": ProcessHandle,
        "mitos:plugin/host@0.1.0.picker": PickerHandle,
        "mitos:plugin/host@0.1.0.picker-row": PickerRowHandle,
        "mitos:plugin/host@0.1.0.prompt": PromptHandle,
        "mitos:plugin/host@0.1.0.writer": WriterHandle,
        "mitos:plugin/host@0.1.0.builtin-group": BuiltinHandle,
        "mitos:plugin/host@0.1.0.keymap-group": KeymapHandle,
        "mitos:plugin/host@0.1.0.keybinding": KeybindingHandle,
    },
);
use mitos::plugin::{host, types};

#[path = "component_ui.rs"]
mod ui;
pub struct PickerHandle {
    invocation: u64,
    action: usize,
    finished: bool,
    bytes: usize,
    children: usize,
}
pub struct PickerRowHandle {
    invocation: u64,
    action: usize,
    row: usize,
    parent: u32,
    fields: u8,
    finished: bool,
}
pub struct PromptHandle {
    invocation: u64,
    action: usize,
    initial_set: bool,
    finished: bool,
}
pub struct WriterHandle {
    invocation: u64,
    target: Option<WriteTarget>,
    bytes: usize,
}
enum WriteTarget {
    File { root: u32, path: String },
    Storage { key: String },
}
pub struct BuiltinHandle {
    invocation: u64,
    action: usize,
    finished: bool,
}
pub struct KeymapHandle {
    invocation: u64,
    action: usize,
    finished: bool,
    children: usize,
}
pub struct KeybindingHandle {
    invocation: u64,
    action: usize,
    binding: usize,
    parent: u32,
    finished: bool,
}
pub struct EffectsHandle {
    invocation: u64,
}
pub struct EditHandle {
    invocation: u64,
    action: usize,
    finished: bool,
}
pub struct SelectionHandle {
    invocation: u64,
    action: usize,
    finished: bool,
}
pub struct JobHandle {
    id: u64,
    job: Arc<dyn HostJob>,
}
struct OwnedJob {
    id: u64,
    job: Arc<dyn HostJob>,
    cancel: CancellationToken,
    watcher: tokio::task::JoinHandle<()>,
}
pub struct ProcessHandle {
    invocation: u64,
    request: Option<JobRequest>,
    bytes: usize,
    input_set: bool,
}

/// Limits account for all core memories/tables in a component, not each alone.
#[derive(Default)]
struct Limits {
    memory: usize,
    table: usize,
    pending_memory: usize,
    pending_table: usize,
    shared_memory: Arc<AtomicUsize>,
    exhausted: bool,
}
impl Drop for Limits {
    fn drop(&mut self) {
        self.shared_memory.fetch_sub(self.memory, Ordering::AcqRel);
    }
}
impl ResourceLimiter for Limits {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let delta = desired.saturating_sub(current);
        if !maximum.is_none_or(|max| desired <= max)
            || self.memory.saturating_add(delta) > MAX_MEMORY_BYTES
        {
            self.exhausted = true;
            return Err(wasmtime::Error::msg(
                "plugin aggregate memory limit exceeded",
            ));
        }
        if self
            .shared_memory
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(delta)
                    .filter(|total| *total <= MAX_TOTAL_MEMORY_BYTES)
            })
            .is_err()
        {
            self.exhausted = true;
            return Err(wasmtime::Error::msg(
                "editor aggregate plugin memory limit exceeded",
            ));
        }
        self.memory += delta;
        self.pending_memory = delta;
        Ok(true)
    }
    fn memory_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.shared_memory
            .fetch_sub(self.pending_memory, Ordering::AcqRel);
        self.memory -= self.pending_memory;
        self.pending_memory = 0;
        Err(error)
    }
    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let delta = desired.saturating_sub(current);
        if !maximum.is_none_or(|max| desired <= max) || self.table.saturating_add(delta) > 4096 {
            self.exhausted = true;
            return Err(wasmtime::Error::msg(
                "plugin aggregate table limit exceeded",
            ));
        }
        self.table += delta;
        self.pending_table = delta;
        Ok(true)
    }
    fn table_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.table -= self.pending_table;
        self.pending_table = 0;
        Err(error)
    }
    fn memories(&self) -> usize {
        8
    }
    fn tables(&self) -> usize {
        8
    }
    fn instances(&self) -> usize {
        16
    }
}

struct HostState {
    generation: u64,
    declared: CapabilitySet,
    granted: CapabilitySet,
    table: ResourceTable,
    limits: Limits,
    services: Option<Arc<dyn HostServices>>,
    cancel: CancellationToken,
    owner_cancel: CancellationToken,
    deadline: Instant,
    invocation: u64,
    target: crate::worker::InvocationTarget,
    job_controls: Arc<JobControls>,
    begun: bool,
    finished: bool,
    open_groups: usize,
    response: Response,
    rejected: Option<ServiceError>,
    effect_bytes: usize,
    entries: usize,
    service_bytes: usize,
    jobs: BTreeMap<u32, OwnedJob>,
    next_job: u64,
}

fn error(code: ErrorCode, message: impl Into<String>) -> ServiceError {
    ServiceError::new(code, message)
}
fn exhausted(message: &str) -> ServiceError {
    error(ErrorCode::ResourceExhausted, message)
}
fn invalid(message: &str) -> ServiceError {
    error(ErrorCode::InvalidRequest, message)
}
fn stale() -> ServiceError {
    error(
        ErrorCode::StaleState,
        "plugin resource belongs to an expired invocation",
    )
}
#[derive(Default)]
pub(crate) struct JobControls {
    entries: Mutex<BTreeMap<u64, (crate::worker::InvocationTarget, CancellationToken)>>,
}
impl JobControls {
    pub(crate) fn target(&self, id: u64) -> Option<crate::worker::InvocationTarget> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&id)
            .filter(|(_, cancel)| !cancel.is_cancelled())
            .map(|(target, _)| *target)
    }
    fn remove(&self, id: u64) {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id);
    }

    fn insert(&self, id: u64, target: crate::worker::InvocationTarget, cancel: CancellationToken) {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id, (target, cancel));
    }
    pub(crate) fn cancel_target(&self, document: Option<u64>, view: Option<u64>) {
        for (target, cancel) in self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
        {
            if document.is_some_and(|id| target.document == Some(id))
                || view.is_some_and(|id| target.view == Some(id))
            {
                cancel.cancel();
            }
        }
    }
}
async fn watch_job(
    job: Arc<dyn HostJob>,
    services: Arc<dyn HostServices>,
    id: u64,
    cancel: CancellationToken,
    controls: Arc<JobControls>,
) {
    let _controls = controls;
    let mut sequence = 0;
    loop {
        let next = tokio::select! { biased; _ = cancel.cancelled() => break, result = job.ready(sequence) => result };
        let next = match next {
            Ok(next) if next > sequence => next,
            Err(error) if error.code == ErrorCode::UnsupportedInterface => {
                cancel.cancelled().await;
                break;
            }
            _ => break,
        };
        let notified = tokio::select! { biased; _ = cancel.cancelled() => break, result = services.notify_job_ready(id) => result };
        if let Err(error) = notified {
            if error.code == ErrorCode::UnsupportedInterface {
                cancel.cancelled().await;
            } else {
                cancel.cancel();
            }
            break;
        }
        sequence = next;
    }
    let _ = job.cancel().await;
}

fn failure(error: ServiceError) -> types::Failure {
    types::Failure {
        code: match error.code {
            ErrorCode::InvalidRequest => types::ErrorCode::InvalidRequest,
            ErrorCode::StaleState => types::ErrorCode::StaleState,
            ErrorCode::PermissionDenied => types::ErrorCode::PermissionDenied,
            ErrorCode::ResourceExhausted => types::ErrorCode::ResourceExhausted,
            ErrorCode::Cancelled => types::ErrorCode::Cancelled,
            ErrorCode::DeadlineExceeded => types::ErrorCode::DeadlineExceeded,
            ErrorCode::UnsupportedInterface => types::ErrorCode::UnsupportedInterface,
            ErrorCode::GuestTrap => types::ErrorCode::GuestTrap,
            ErrorCode::HostFailure => types::ErrorCode::HostFailure,
        },
        message: truncate(error.message, MAX_ERROR_BYTES),
    }
}
fn service_error(fail: types::Failure) -> ServiceError {
    let code = match fail.code {
        types::ErrorCode::InvalidRequest => ErrorCode::InvalidRequest,
        types::ErrorCode::StaleState => ErrorCode::StaleState,
        types::ErrorCode::PermissionDenied => ErrorCode::PermissionDenied,
        types::ErrorCode::ResourceExhausted => ErrorCode::ResourceExhausted,
        types::ErrorCode::Cancelled => ErrorCode::Cancelled,
        types::ErrorCode::DeadlineExceeded => ErrorCode::DeadlineExceeded,
        types::ErrorCode::UnsupportedInterface => ErrorCode::UnsupportedInterface,
        types::ErrorCode::GuestTrap => ErrorCode::GuestTrap,
        types::ErrorCode::HostFailure => ErrorCode::HostFailure,
    };
    error(code, truncate(fail.message, MAX_ERROR_BYTES))
}
fn truncate(mut text: String, limit: usize) -> String {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

impl HostState {
    fn live(&self) -> Result<(), ServiceError> {
        if self.cancel.is_cancelled() {
            return Err(error(ErrorCode::Cancelled, "plugin invocation cancelled"));
        }
        if Instant::now() >= self.deadline {
            return Err(error(
                ErrorCode::DeadlineExceeded,
                "plugin invocation deadline exceeded",
            ));
        }
        Ok(())
    }
    fn require(&self, capability: Capability) -> Result<(), ServiceError> {
        self.live()?;
        capability.require(&self.declared, &self.granted)
    }
    fn builder(&self, invocation: u64) -> Result<(), ServiceError> {
        self.live()?;
        if self.invocation != invocation || !self.begun || self.finished {
            return Err(stale());
        }
        if let Some(error) = &self.rejected {
            return Err(error.clone());
        }
        Ok(())
    }
    fn reject<T>(&mut self, result: Result<T, ServiceError>) -> Result<T, types::Failure> {
        result.map_err(|error| {
            self.rejected.get_or_insert_with(|| error.clone());
            failure(error)
        })
    }
    fn reserve(&mut self, bytes: usize, entries: usize) -> Result<(), ServiceError> {
        if self.effect_bytes.saturating_add(bytes) > MAX_MESSAGE_BYTES
            || self.entries.saturating_add(entries) > MAX_ENTRIES
        {
            return Err(exhausted(
                "plugin effects exceed the retained byte or entry limit",
            ));
        }
        self.effect_bytes += bytes;
        self.entries += entries;
        Ok(())
    }
    fn action(&mut self, action: Action, bytes: usize) -> Result<usize, ServiceError> {
        if self.response.actions.len() >= MAX_ACTIONS {
            return Err(exhausted("plugin effect count exceeded"));
        }
        self.reserve(bytes, 1)?;
        let index = self.response.actions.len();
        self.response.actions.push(action);
        Ok(index)
    }
    async fn await_service<T>(
        &mut self,
        future: plugin_api::HostFuture<T>,
    ) -> Result<T, ServiceError> {
        self.live()?;
        tokio::select! {
            _ = self.cancel.cancelled() => Err(error(ErrorCode::Cancelled, "plugin service cancelled")),
            _ = tokio::time::sleep_until(self.deadline.into()) => Err(error(ErrorCode::DeadlineExceeded, "plugin service deadline exceeded")),
            result = future => result,
        }
    }
    async fn start(&mut self, request: JobRequest) -> Result<Resource<JobHandle>, ServiceError> {
        self.live()?;
        if self.jobs.len() >= MAX_JOBS {
            return Err(exhausted("plugin job count exceeded"));
        }
        match &request {
            JobRequest::Timer { milliseconds } => {
                if *milliseconds > 60_000 {
                    return Err(exhausted("plugin timer exceeds one minute"));
                }
            }
            JobRequest::Search { .. } => self.require(Capability::WorkspaceRead)?,
            JobRequest::Process { .. } => self.require(Capability::Process)?,
        }
        let services = self.services.as_ref().ok_or_else(stale)?.clone();
        // The service contract makes creation cancellation-safe. When ownership
        // is returned in a cancellation race, revoke the registered job before
        // admitting its resource into the guest table.
        let result = self.await_service(services.start_job(request)).await;
        let job = result?;
        if let Err(error) = self.live() {
            let _ = job.cancel().await;
            return Err(error);
        }
        self.next_job = self
            .next_job
            .checked_add(1)
            .ok_or_else(|| exhausted("plugin job identifiers exhausted"))?;
        let id = self.next_job;
        match self.table.push(JobHandle {
            id,
            job: job.clone(),
        }) {
            Ok(handle) => {
                let cancel = self.owner_cancel.child_token();
                self.job_controls.insert(id, self.target, cancel.clone());
                let watcher = tokio::spawn(watch_job(
                    job.clone(),
                    services,
                    id,
                    cancel.clone(),
                    self.job_controls.clone(),
                ));
                self.jobs.insert(
                    handle.rep(),
                    OwnedJob {
                        id,
                        job,
                        cancel,
                        watcher,
                    },
                );
                Ok(handle)
            }
            Err(_) => {
                let _ = job.cancel().await;
                Err(exhausted("plugin resource handle count exceeded"))
            }
        }
    }
    async fn cleanup_jobs(&mut self) {
        let jobs = std::mem::take(&mut self.jobs);
        for job in jobs.values() {
            job.cancel.cancel();
        }
        for (_, owned) in jobs {
            self.job_controls.remove(owned.id);
            let _ = owned.job.cancel().await;
            let _ = owned.watcher.await;
        }
    }
    async fn stop_job(&mut self, rep: u32, job: Arc<dyn HostJob>) -> Result<(), ServiceError> {
        let owned = self.jobs.remove(&rep);
        if let Some(owned) = &owned {
            self.job_controls.remove(owned.id);
            owned.cancel.cancel();
        }
        let result = job.cancel().await;
        if let Some(owned) = owned {
            let _ = owned.watcher.await;
        }
        result
    }
    fn writer(&mut self, target: WriteTarget) -> Result<Resource<WriterHandle>, ServiceError> {
        let bytes = match &target {
            WriteTarget::File { path, .. } => path.len(),
            WriteTarget::Storage { key } => key.len(),
        };
        if self.service_bytes.saturating_add(bytes) > MAX_MESSAGE_BYTES {
            return Err(exhausted(
                "plugin pending service requests exceed their byte limit",
            ));
        }
        let handle = self
            .table
            .push(WriterHandle {
                invocation: self.invocation,
                target: Some(target),
                bytes,
            })
            .map_err(|_| exhausted("plugin resource handle count exceeded"))?;
        self.service_bytes += bytes;
        Ok(handle)
    }
}
impl types::Host for HostState {}
impl host::Host for HostState {
    async fn read_document(
        &mut self,
        document: u64,
        version: i32,
        start: u64,
        end: u64,
    ) -> wasmtime::Result<Result<String, types::Failure>> {
        let result = async {
            self.require(Capability::EditorRead)?;
            if start > end {
                return Err(invalid("document read range is reversed"));
            }
            let services = self.services.as_ref().ok_or_else(stale)?.clone();
            let text = self
                .await_service(services.read_document(ReadRequest {
                    generation: self.generation,
                    document,
                    version,
                    start,
                    end,
                    max_bytes: MAX_MESSAGE_BYTES,
                }))
                .await?;
            if text.len() > MAX_MESSAGE_BYTES {
                return Err(exhausted("host document read exceeds its byte limit"));
            }
            Ok(text)
        }
        .await;
        Ok(result.map_err(failure))
    }
    async fn begin_effects(
        &mut self,
    ) -> wasmtime::Result<Result<Resource<EffectsHandle>, types::Failure>> {
        let result = (|| {
            self.live()?;
            if self.begun {
                return Err(invalid("only one effect batch is permitted per invocation"));
            }
            let handle = self
                .table
                .push(EffectsHandle {
                    invocation: self.invocation,
                })
                .map_err(|_| exhausted("plugin resource handle count exceeded"))?;
            self.begun = true;
            Ok(handle)
        })();
        Ok(self.reject(result))
    }
    async fn start_timer(
        &mut self,
        milliseconds: u64,
    ) -> wasmtime::Result<Result<Resource<JobHandle>, types::Failure>> {
        Ok(self
            .start(JobRequest::Timer { milliseconds })
            .await
            .map_err(failure))
    }
    async fn start_search(
        &mut self,
        root: u32,
        query: String,
    ) -> wasmtime::Result<Result<Resource<JobHandle>, types::Failure>> {
        if query.len() > MAX_ERROR_BYTES {
            return Ok(Err(failure(exhausted("search query is too long"))));
        }
        Ok(self
            .start(JobRequest::Search { root, query })
            .await
            .map_err(failure))
    }
    async fn process(
        &mut self,
        command: String,
        root: u32,
    ) -> wasmtime::Result<Result<Resource<ProcessHandle>, types::Failure>> {
        let result = (|| {
            self.require(Capability::Process)?;
            let bytes = command.len();
            if command.is_empty()
                || command.len() > MAX_ERROR_BYTES
                || self.service_bytes.saturating_add(bytes) > MAX_MESSAGE_BYTES
            {
                return Err(exhausted("plugin process request exceeds its byte limit"));
            }
            let handle = self
                .table
                .push(ProcessHandle {
                    invocation: self.invocation,
                    request: Some(JobRequest::Process {
                        command,
                        input: String::new(),
                        root,
                        args: Vec::new(),
                    }),
                    bytes,
                    input_set: false,
                })
                .map_err(|_| exhausted("plugin resource handle count exceeded"))?;
            self.service_bytes += bytes;
            Ok(handle)
        })();
        Ok(result.map_err(failure))
    }
    async fn editor_request(
        &mut self,
        payload: String,
    ) -> wasmtime::Result<Result<String, types::Failure>> {
        let result = async {
            self.live()?;
            if payload.len() > 4096 {
                return Err(exhausted("plugin editor request exceeds its byte limit"));
            }
            let request: plugin_api::editor::EditorRequest = serde_json::from_str(&payload)
                .map_err(|_| invalid("plugin editor request is not a supported typed request"))?;
            request.validate()?;
            for capability in request.capabilities() {
                self.require(capability)?;
            }
            let services = self.services.as_ref().ok_or_else(stale)?.clone();
            let reply = self.await_service(services.editor_request(request)).await?;
            bounded_json(&reply, 1024 * 1024)
        }
        .await;
        Ok(result.map_err(failure))
    }
    async fn read_file(
        &mut self,
        root: u32,
        path: String,
    ) -> wasmtime::Result<Result<String, types::Failure>> {
        let result = async {
            self.require(Capability::WorkspaceRead)?;
            if path.is_empty() || path.len() > MAX_ERROR_BYTES {
                return Err(invalid("plugin file path is invalid"));
            }
            let services = self.services.as_ref().ok_or_else(stale)?.clone();
            let value = self.await_service(services.read_file(root, path)).await?;
            if value.len() > MAX_MESSAGE_BYTES {
                return Err(exhausted("plugin file read exceeds its byte limit"));
            }
            Ok(value)
        }
        .await;
        Ok(result.map_err(failure))
    }
    async fn write_file(
        &mut self,
        root: u32,
        path: String,
    ) -> wasmtime::Result<Result<Resource<WriterHandle>, types::Failure>> {
        let result = (|| {
            self.require(Capability::WorkspaceWrite)?;
            if path.is_empty() || path.len() > MAX_ERROR_BYTES {
                return Err(invalid("plugin file path is invalid"));
            }
            self.writer(WriteTarget::File { root, path })
        })();
        Ok(result.map_err(failure))
    }
    async fn storage_read(
        &mut self,
        key: String,
    ) -> wasmtime::Result<Result<Option<String>, types::Failure>> {
        let result = async {
            self.require(Capability::Storage)?;
            if key.is_empty() || key.len() > MAX_KEY_BYTES {
                return Err(invalid("plugin storage key is invalid"));
            }
            let services = self.services.as_ref().ok_or_else(stale)?.clone();
            let value = self.await_service(services.storage_read(key)).await?;
            if value
                .as_ref()
                .is_some_and(|value| value.len() > MAX_MESSAGE_BYTES)
            {
                return Err(exhausted("stored value exceeds its byte limit"));
            }
            Ok(value)
        }
        .await;
        Ok(result.map_err(failure))
    }
    async fn storage_write(
        &mut self,
        key: String,
    ) -> wasmtime::Result<Result<Resource<WriterHandle>, types::Failure>> {
        let result = (|| {
            self.require(Capability::Storage)?;
            if key.is_empty() || key.len() > MAX_KEY_BYTES {
                return Err(invalid("plugin storage key is invalid"));
            }
            self.writer(WriteTarget::Storage { key })
        })();
        Ok(result.map_err(failure))
    }
}

impl host::HostEffects for HostState {
    async fn edit(
        &mut self,
        batch: Resource<EffectsHandle>,
        document: u64,
        version: i32,
    ) -> wasmtime::Result<Result<Resource<EditHandle>, types::Failure>> {
        let result = (|| {
            self.builder(self.table.get(&batch).map_err(|_| stale())?.invocation)?;
            self.require(Capability::EditorEdit)?;
            let handle = self
                .table
                .push(EditHandle {
                    invocation: self.invocation,
                    action: self.response.actions.len(),
                    finished: false,
                })
                .map_err(|_| exhausted("plugin resource handle count exceeded"))?;
            if let Err(error) = self.action(
                Action::Edit {
                    document,
                    version,
                    edits: Vec::new(),
                },
                32,
            ) {
                let _ = self.table.delete(handle);
                return Err(error);
            }
            self.open_groups += 1;
            Ok(handle)
        })();
        Ok(self.reject(result))
    }
    async fn selection(
        &mut self,
        batch: Resource<EffectsHandle>,
        document: u64,
        version: i32,
        view: u64,
        binding_revision: u64,
        selection_revision: u64,
        primary: u32,
    ) -> wasmtime::Result<Result<Resource<SelectionHandle>, types::Failure>> {
        let result = (|| {
            self.builder(self.table.get(&batch).map_err(|_| stale())?.invocation)?;
            self.require(Capability::EditorSelection)?;
            let handle = self
                .table
                .push(SelectionHandle {
                    invocation: self.invocation,
                    action: self.response.actions.len(),
                    finished: false,
                })
                .map_err(|_| exhausted("plugin resource handle count exceeded"))?;
            if let Err(error) = self.action(
                Action::SetSelection {
                    document,
                    version,
                    view,
                    binding_revision,
                    selection_revision,
                    primary: primary as usize,
                    ranges: Vec::new(),
                },
                64,
            ) {
                let _ = self.table.delete(handle);
                return Err(error);
            }
            self.open_groups += 1;
            Ok(handle)
        })();
        Ok(self.reject(result))
    }
    async fn status(
        &mut self,
        batch: Resource<EffectsHandle>,
        message: String,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            self.builder(self.table.get(&batch).map_err(|_| stale())?.invocation)?;
            self.require(Capability::Ui)?;
            let bytes = message.len();
            self.action(Action::Status { message }, bytes)?;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn error(
        &mut self,
        batch: Resource<EffectsHandle>,
        message: String,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            self.builder(self.table.get(&batch).map_err(|_| stale())?.invocation)?;
            self.require(Capability::Ui)?;
            let bytes = message.len();
            self.action(Action::Error { message }, bytes)?;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn open(
        &mut self,
        batch: Resource<EffectsHandle>,
        path: String,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            self.builder(self.table.get(&batch).map_err(|_| stale())?.invocation)?;
            self.require(Capability::EditorNavigate)?;
            if path.is_empty() || path.len() > MAX_ERROR_BYTES {
                return Err(invalid("plugin open path is invalid"));
            }
            let bytes = path.len();
            self.action(Action::Open { path }, bytes)?;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn request_state(
        &mut self,
        batch: Resource<EffectsHandle>,
        query: types::StateQuery,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            self.builder(self.table.get(&batch).map_err(|_| stale())?.invocation)?;
            self.require(Capability::EditorRead)?;
            if query.limit > 64 {
                return Err(exhausted("state query page exceeds 64 entries"));
            }
            self.action(
                Action::RequestState {
                    query: StateQuery {
                        document: query.document,
                        view: query.view,
                        after_document: query.after_document,
                        after_view: query.after_view,
                        limit: query.limit as usize,
                    },
                },
                64,
            )?;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn prompt(
        &mut self,
        batch: Resource<EffectsHandle>,
        request: u64,
        origin: Option<types::UiOrigin>,
        title: String,
    ) -> wasmtime::Result<Result<Resource<PromptHandle>, types::Failure>> {
        let result = self.ui_prompt(batch, request, origin, title);
        Ok(self.reject(result))
    }
    async fn next_key(
        &mut self,
        batch: Resource<EffectsHandle>,
        request: u64,
        origin: Option<types::UiOrigin>,
        title: String,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = self.ui_next_key(batch, request, origin, title, timeout_ms);
        Ok(self.reject(result))
    }
    async fn picker(
        &mut self,
        batch: Resource<EffectsHandle>,
        request: u64,
        origin: Option<types::UiOrigin>,
        title: String,
    ) -> wasmtime::Result<Result<Resource<PickerHandle>, types::Failure>> {
        let result = self.ui_picker(batch, request, origin, title);
        Ok(self.reject(result))
    }
    async fn builtins(
        &mut self,
        batch: Resource<EffectsHandle>,
        request: u64,
        origin: types::UiOrigin,
    ) -> wasmtime::Result<Result<Resource<BuiltinHandle>, types::Failure>> {
        let result = self.ui_builtins(batch, request, origin);
        Ok(self.reject(result))
    }
    async fn keymap(
        &mut self,
        batch: Resource<EffectsHandle>,
        request: u64,
    ) -> wasmtime::Result<Result<Resource<KeymapHandle>, types::Failure>> {
        let result = self.ui_keymap(batch, request);
        Ok(self.reject(result))
    }
    async fn finish(
        &mut self,
        batch: Resource<EffectsHandle>,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            self.builder(self.table.get(&batch).map_err(|_| stale())?.invocation)?;
            if self.open_groups != 0 {
                return Err(invalid("plugin effects contain unfinished groups"));
            }
            self.finished = true;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn drop(&mut self, batch: Resource<EffectsHandle>) -> wasmtime::Result<()> {
        let batch = self.table.delete(batch)?;
        if batch.invocation == self.invocation && !self.finished {
            self.rejected
                .get_or_insert_with(|| invalid("plugin dropped an unfinished effect batch"));
        }
        Ok(())
    }
}

impl host::HostEditGroup for HostState {
    async fn add(
        &mut self,
        group: Resource<EditHandle>,
        start: u64,
        end: u64,
        text: String,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let handle = self.table.get(&group).map_err(|_| stale())?;
            self.builder(handle.invocation)?;
            if handle.finished || start > end {
                return Err(invalid("plugin edit group or range is invalid"));
            }
            let action = handle.action;
            let start =
                usize::try_from(start).map_err(|_| invalid("edit offset exceeds host range"))?;
            let end =
                usize::try_from(end).map_err(|_| invalid("edit offset exceeds host range"))?;
            self.reserve(text.len().saturating_add(24), 1)?;
            match &mut self.response.actions[action] {
                Action::Edit { edits, .. } => edits.push(TextEdit { start, end, text }),
                _ => unreachable!(),
            }
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn finish(
        &mut self,
        group: Resource<EditHandle>,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let handle = self.table.get(&group).map_err(|_| stale())?;
            self.builder(handle.invocation)?;
            if handle.finished {
                return Err(invalid("edit group already finished"));
            }
            self.table.get_mut(&group).map_err(|_| stale())?.finished = true;
            self.open_groups -= 1;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn drop(&mut self, group: Resource<EditHandle>) -> wasmtime::Result<()> {
        let group = self.table.delete(group)?;
        if group.invocation == self.invocation && !group.finished {
            self.open_groups -= 1;
            self.rejected
                .get_or_insert_with(|| invalid("plugin dropped an unfinished edit group"));
        }
        Ok(())
    }
}
impl host::HostSelectionGroup for HostState {
    async fn add(
        &mut self,
        group: Resource<SelectionHandle>,
        anchor: u64,
        head: u64,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let handle = self.table.get(&group).map_err(|_| stale())?;
            self.builder(handle.invocation)?;
            if handle.finished {
                return Err(invalid("selection group already finished"));
            }
            let action = handle.action;
            let anchor = usize::try_from(anchor)
                .map_err(|_| invalid("selection offset exceeds host range"))?;
            let head = usize::try_from(head)
                .map_err(|_| invalid("selection offset exceeds host range"))?;
            self.reserve(16, 1)?;
            match &mut self.response.actions[action] {
                Action::SetSelection { ranges, .. } => ranges.push(SelectionRange { anchor, head }),
                _ => unreachable!(),
            }
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn finish(
        &mut self,
        group: Resource<SelectionHandle>,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let handle = self.table.get(&group).map_err(|_| stale())?;
            self.builder(handle.invocation)?;
            if handle.finished {
                return Err(invalid("selection group already finished"));
            }
            let Action::SetSelection {
                ranges, primary, ..
            } = &self.response.actions[handle.action]
            else {
                unreachable!()
            };
            if ranges.is_empty() || *primary >= ranges.len() {
                return Err(invalid("selection group has no valid primary range"));
            }
            self.table.get_mut(&group).map_err(|_| stale())?.finished = true;
            self.open_groups -= 1;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn drop(&mut self, group: Resource<SelectionHandle>) -> wasmtime::Result<()> {
        let group = self.table.delete(group)?;
        if group.invocation == self.invocation && !group.finished {
            self.open_groups -= 1;
            self.rejected
                .get_or_insert_with(|| invalid("plugin dropped an unfinished selection group"));
        }
        Ok(())
    }
}
impl host::HostJob for HostState {
    async fn id(&mut self, job: Resource<JobHandle>) -> wasmtime::Result<u64> {
        Ok(self.table.get(&job)?.id)
    }
    async fn poll(
        &mut self,
        job: Resource<JobHandle>,
    ) -> wasmtime::Result<Result<String, types::Failure>> {
        let result = async {
            self.live()?;
            let job = self.table.get(&job).map_err(|_| stale())?.job.clone();
            let output = self.await_service(job.poll()).await?;
            if matches!(output, plugin_api::JobPoll::Pending) {
                tokio::task::yield_now().await;
            }
            bounded_json(&output, MAX_MESSAGE_BYTES)
        }
        .await;
        Ok(result.map_err(failure))
    }
    async fn cancel(
        &mut self,
        job: Resource<JobHandle>,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let rep = job.rep();
        let job = self.table.get(&job).map_err(|_| stale())?.job.clone();
        Ok(self.stop_job(rep, job).await.map_err(failure))
    }
    async fn drop(&mut self, job: Resource<JobHandle>) -> wasmtime::Result<()> {
        let rep = job.rep();
        let job = self.table.delete(job)?;
        self.stop_job(rep, job.job)
            .await
            .map_err(|error| wasmtime::Error::msg(error.message))
    }
}
impl host::HostProcessRequest for HostState {
    async fn input(
        &mut self,
        process: Resource<ProcessHandle>,
        value: String,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            self.require(Capability::Process)?;
            let handle = self.table.get(&process).map_err(|_| stale())?;
            if handle.invocation != self.invocation || handle.input_set || handle.request.is_none()
            {
                return Err(stale());
            }
            if self.service_bytes.saturating_add(value.len()) > MAX_MESSAGE_BYTES {
                return Err(exhausted("plugin process input exceeds its byte limit"));
            }
            self.service_bytes += value.len();
            let handle = self.table.get_mut(&process).map_err(|_| stale())?;
            handle.bytes += value.len();
            handle.input_set = true;
            let Some(JobRequest::Process { input, .. }) = &mut handle.request else {
                unreachable!()
            };
            *input = value;
            Ok(())
        })();
        Ok(result.map_err(failure))
    }
    async fn argument(
        &mut self,
        process: Resource<ProcessHandle>,
        value: String,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            self.require(Capability::Process)?;
            let handle = self.table.get(&process).map_err(|_| stale())?;
            if handle.invocation != self.invocation {
                return Err(stale());
            }
            let Some(JobRequest::Process { args, .. }) = &handle.request else {
                return Err(stale());
            };
            if args.len() >= MAX_ARGUMENTS
                || value.len() > MAX_ERROR_BYTES
                || self.service_bytes.saturating_add(value.len()) > MAX_MESSAGE_BYTES
            {
                return Err(exhausted("plugin process arguments exceed their limit"));
            }
            self.service_bytes += value.len();
            let handle = self.table.get_mut(&process).map_err(|_| stale())?;
            handle.bytes += value.len();
            let Some(JobRequest::Process { args, .. }) = &mut handle.request else {
                unreachable!()
            };
            args.push(value);
            Ok(())
        })();
        Ok(result.map_err(failure))
    }
    async fn start(
        &mut self,
        process: Resource<ProcessHandle>,
    ) -> wasmtime::Result<Result<Resource<JobHandle>, types::Failure>> {
        let result = async {
            self.require(Capability::Process)?;
            let handle = self.table.get_mut(&process).map_err(|_| stale())?;
            if handle.invocation != self.invocation {
                return Err(stale());
            }
            let request = handle.request.take().ok_or_else(stale)?;
            self.service_bytes -= handle.bytes;
            handle.bytes = 0;
            self.start(request).await
        }
        .await;
        Ok(result.map_err(failure))
    }
    async fn drop(&mut self, process: Resource<ProcessHandle>) -> wasmtime::Result<()> {
        let handle = self.table.delete(process)?;
        self.service_bytes -= handle.bytes;
        Ok(())
    }
}
impl host::HostWriter for HostState {
    async fn write(
        &mut self,
        handle: Resource<WriterHandle>,
        value: String,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = async {
            self.live()?;
            if value.len() > MAX_MESSAGE_BYTES {
                return Err(exhausted("plugin write value exceeds its byte limit"));
            }
            let writer = self.table.get(&handle).map_err(|_| stale())?;
            if writer.invocation != self.invocation {
                return Err(stale());
            }
            let capability = match &writer.target {
                Some(WriteTarget::File { .. }) => Capability::WorkspaceWrite,
                Some(WriteTarget::Storage { .. }) => Capability::Storage,
                None => return Err(stale()),
            };
            self.require(capability)?;
            let writer = self.table.get_mut(&handle).map_err(|_| stale())?;
            let target = writer.target.take().ok_or_else(stale)?;
            self.service_bytes -= writer.bytes;
            writer.bytes = 0;
            let services = self.services.as_ref().ok_or_else(stale)?.clone();
            let operation = match target {
                WriteTarget::File { root, path } => services.write_file(root, path, value),
                WriteTarget::Storage { key } => services.storage_write(key, value),
            };
            self.await_service(operation).await
        }
        .await;
        Ok(result.map_err(failure))
    }
    async fn drop(&mut self, handle: Resource<WriterHandle>) -> wasmtime::Result<()> {
        let writer = self.table.delete(handle)?;
        self.service_bytes -= writer.bytes;
        Ok(())
    }
}
impl wasmtime::component::HasData for HostState {
    type Data<'a> = &'a mut HostState;
}

pub(crate) fn engine() -> Result<Engine, ServiceError> {
    let mut config = Config::new();
    // The editor spawns native children and handles process signals. Wasmtime's
    // supported Unix trap path avoids the macOS Mach helper's receive aborts.
    #[cfg(target_os = "macos")]
    config.macos_use_mach_ports(false);
    config
        .wasm_component_model(true)
        .epoch_interruption(true)
        .wasm_simd(false)
        .wasm_relaxed_simd(false)
        .max_wasm_stack(1024 * 1024)
        .async_stack_size(2 * 1024 * 1024)
        .memory_reservation(MAX_MEMORY_BYTES as u64)
        .memory_reservation_for_growth(0)
        .memory_guard_size(65536);
    Engine::new(&config).map_err(|cause| {
        error(
            ErrorCode::HostFailure,
            format!("creating plugin engine: {cause}"),
        )
    })
}

pub(crate) struct ExecutionFailure {
    pub error: ServiceError,
    pub discard: bool,
}
impl From<ServiceError> for ExecutionFailure {
    fn from(error: ServiceError) -> Self {
        Self {
            error,
            discard: false,
        }
    }
}
pub(crate) struct Instance {
    binding: Plugin,
    store: Store<HostState>,
    _code: Arc<crate::worker::Compiled>,
}
pub(crate) struct InstanceOptions {
    pub generation: u64,
    pub declared: CapabilitySet,
    pub granted: CapabilitySet,
    pub cancel: CancellationToken,
    pub deadline: Instant,
    pub shared_memory: Arc<AtomicUsize>,
}
pub(crate) struct Invocation {
    pub request: ApiRequest,
    pub services: Arc<dyn HostServices>,
    pub cancel: CancellationToken,
    pub owner_cancel: CancellationToken,
    pub deadline: Instant,
    pub target: crate::worker::InvocationTarget,
    pub job_controls: Arc<JobControls>,
    pub config_json: Option<Arc<str>>,
}
impl Instance {
    pub(crate) async fn new(
        engine: &Engine,
        component: &Arc<crate::worker::Compiled>,
        options: InstanceOptions,
    ) -> Result<Self, ServiceError> {
        let InstanceOptions {
            generation,
            declared,
            granted,
            cancel,
            deadline,
            shared_memory,
        } = options;
        let mut linker = Linker::new(engine);
        Plugin::add_to_linker::<_, HostState>(&mut linker, |state| state)
            .map_err(|error| ServiceError::new(ErrorCode::HostFailure, error.to_string()))?;
        let linked = linker
            .instantiate_pre(&component.component)
            .map_err(|error| {
                ServiceError::new(
                    ErrorCode::UnsupportedInterface,
                    format!("linking plugin component imports: {error}"),
                )
            })?;
        let binding = PluginPre::new(linked).map_err(|error| {
            ServiceError::new(
                ErrorCode::UnsupportedInterface,
                format!("checking plugin component exports: {error}"),
            )
        })?;
        let mut table = ResourceTable::new();
        table.set_max_capacity(MAX_HANDLES);
        let mut store = Store::new(
            engine,
            HostState {
                generation,
                declared,
                granted,
                table,
                limits: Limits {
                    shared_memory,
                    ..Limits::default()
                },
                services: None,
                cancel,
                owner_cancel: CancellationToken::new(),
                deadline,
                invocation: 0,
                target: crate::worker::InvocationTarget::default(),
                job_controls: Arc::new(JobControls::default()),
                begun: false,
                finished: false,
                open_groups: 0,
                response: Response::default(),
                rejected: None,
                effect_bytes: 0,
                entries: 0,
                service_bytes: 0,
                jobs: BTreeMap::new(),
                next_job: 0,
            },
        );
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(|context| {
            let state = context.data();
            state
                .live()
                .map_err(|error| wasmtime::Error::msg(error.message))?;
            Ok(UpdateDeadline::Yield(1))
        });
        let binding = binding
            .instantiate_async(&mut store)
            .await
            .map_err(|error| {
                let state = store.data();
                let code = if state.cancel.is_cancelled() {
                    ErrorCode::Cancelled
                } else if Instant::now() >= state.deadline {
                    ErrorCode::DeadlineExceeded
                } else if state.limits.exhausted {
                    ErrorCode::ResourceExhausted
                } else {
                    ErrorCode::GuestTrap
                };
                ServiceError::new(code, format!("instantiating plugin component: {error}"))
            })?;
        Ok(Self {
            binding,
            store,
            _code: component.clone(),
        })
    }
    pub(crate) async fn call(
        &mut self,
        invocation: Invocation,
    ) -> Result<Response, ExecutionFailure> {
        let Invocation {
            request,
            services,
            cancel,
            owner_cancel,
            deadline,
            target,
            job_controls,
            config_json,
        } = invocation;
        if request.editor.generation != self.store.data().generation {
            return Err(stale().into());
        }
        let request = transport(request, config_json.as_deref())?;
        let state = self.store.data_mut();
        state.invocation += 1;
        state.services = Some(services);
        state.cancel = cancel;
        state.owner_cancel = owner_cancel;
        state.target = target;
        state.job_controls = job_controls;
        state.deadline = deadline;
        state.begun = false;
        state.finished = false;
        state.open_groups = 0;
        state.response = Response::default();
        state.limits.exhausted = false;
        state.rejected = None;
        state.effect_bytes = 0;
        state.entries = 0;
        self.store.set_epoch_deadline(1);
        let result = self.binding.call_handle(&mut self.store, &request).await;
        self.store.data_mut().services = None;
        match result {
            Err(trap) => {
                let state = self.store.data();
                let code = if state.cancel.is_cancelled() {
                    ErrorCode::Cancelled
                } else if Instant::now() >= state.deadline {
                    ErrorCode::DeadlineExceeded
                } else if state.limits.exhausted {
                    ErrorCode::ResourceExhausted
                } else {
                    ErrorCode::GuestTrap
                };
                let error = error(
                    code,
                    truncate(format!("plugin execution failed: {trap}"), MAX_ERROR_BYTES),
                );
                self.cleanup().await;
                Err(ExecutionFailure {
                    error,
                    discard: true,
                })
            }
            Ok(Err(guest_error)) => {
                self.store.data_mut().response = Response::default();
                Err(service_error(guest_error).into())
            }
            Ok(Ok(())) => {
                let state = self.store.data_mut();
                if let Some(error) = state.rejected.take() {
                    state.response = Response::default();
                    return Err(error.into());
                }
                if state.begun && !state.finished {
                    state.response = Response::default();
                    return Err(invalid("plugin did not finish its effect batch").into());
                }
                state.live()?;
                Ok(std::mem::take(&mut state.response))
            }
        }
    }
    pub(crate) async fn cleanup(&mut self) {
        self.store.data_mut().cancel.cancel();
        self.store.data_mut().owner_cancel.cancel();
        self.store.data_mut().cleanup_jobs().await;
        self.store.data_mut().response = Response::default();
        self.store.data_mut().services = None;
    }
}

fn transport(request: ApiRequest, config: Option<&str>) -> Result<types::Request, ServiceError> {
    let event = match request.event {
        Event::Init => types::Event::Init,
        Event::Shutdown => types::Event::Shutdown,
        Event::Command => types::Event::Command,
        Event::DocumentOpened => types::Event::DocumentOpened,
        Event::DocumentChanged => types::Event::DocumentChanged,
        Event::DocumentSaved => types::Event::DocumentSaved,
        Event::DocumentClosed => types::Event::DocumentClosed,
        Event::SelectionChanged => types::Event::SelectionChanged,
        Event::ModeChanged => types::Event::ModeChanged,
        Event::PostCommand => types::Event::PostCommand,
        Event::PostInsertChar => types::Event::PostInsertChar,
        Event::DocumentFocusLost => types::Event::DocumentFocusLost,
        Event::TerminalFocusGained => types::Event::TerminalFocusGained,
        Event::TerminalFocusLost => types::Event::TerminalFocusLost,
        Event::ResyncRequired => types::Event::ResyncRequired,
        Event::State => types::Event::State,
        Event::UiResult => types::Event::UiResult,
        Event::BuiltinResult => types::Event::BuiltinResult,
        Event::KeymapResult => types::Event::KeymapResult,
        Event::JobReady => types::Event::JobReady,
    };
    Ok(types::Request {
        abi_version: request.abi_version,
        event,
        command: request.command,
        args: request.args,
        config_json: match config {
            Some(config) => config.to_owned(),
            None => bounded_json(&request.config, 64 * 1024)?,
        },
        data_json: bounded_json(&request.data, MAX_MESSAGE_BYTES)?,
        editor: types::EditorContext {
            generation: request.editor.generation,
            mode: request.editor.mode,
            document: request.editor.document.map(|doc| types::Document {
                id: doc.id,
                version: doc.version,
                path: doc.path,
                language: doc.language,
                char_count: doc.char_count,
                byte_count: doc.byte_count,
            }),
            view: request.editor.view.map(|view| types::View {
                id: view.id,
                document: view.document,
                binding_revision: view.binding_revision,
                selection_revision: view.selection_revision,
                selections: view
                    .selections
                    .into_iter()
                    .map(|range| types::SelectionRange {
                        anchor: range.anchor as u64,
                        head: range.head as u64,
                    })
                    .collect(),
                primary: view.primary as u32,
            }),
        },
    })
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            return Err(std::io::Error::other("plugin message exceeds byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) fn bounded_json(
    value: &impl serde::Serialize,
    limit: usize,
) -> Result<String, ServiceError> {
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| exhausted("plugin JSON value exceeds its byte limit"))?;
    String::from_utf8(writer.bytes).map_err(|_| invalid("plugin JSON is not UTF-8"))
}

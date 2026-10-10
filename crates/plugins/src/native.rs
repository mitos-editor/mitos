//! Explicit native host jobs. Process grants authorize native execution, not a
//! filesystem or network sandbox for the executable.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use plugin_api::{
    Capability, ErrorCode, HostFuture, HostJob, HostServices, JobOutput, JobPoll, JobRequest,
    ReadRequest, ReadRoot, SearchMatch, ServiceError,
};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, watch, Semaphore},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{filesystem::ScopedDirectory, policy::AccessPolicy};

const MAX_JOBS: usize = 16;
const MAX_EDITOR_JOBS: usize = 32;
const MAX_OUTPUT: usize = 1024 * 1024;
const MAX_STORAGE_VALUE: usize = 64 * 1024;
const MAX_STORAGE_BYTES: u64 = 1024 * 1024;
const MAX_STORAGE_KEYS: usize = 128;
const JOB_DEADLINE: Duration = Duration::from_secs(10);

fn failure(code: ErrorCode, message: impl Into<String>) -> ServiceError {
    ServiceError::new(code, message)
}
fn host_failure(error: impl std::fmt::Display) -> ServiceError {
    failure(ErrorCode::HostFailure, error.to_string())
}
fn cancelled() -> ServiceError {
    failure(ErrorCode::Cancelled, "plugin job was cancelled")
}

/// Share across all packages and replacement generations in one editor.
#[derive(Clone)]
pub struct NativeBudget {
    jobs: Arc<Semaphore>,
    io: Arc<Semaphore>,
}

impl Default for NativeBudget {
    fn default() -> Self {
        Self {
            jobs: Arc::new(Semaphore::new(MAX_EDITOR_JOBS)),
            io: Arc::new(Semaphore::new(1)),
        }
    }
}

/// Reused across a generation; editor reads remain in the editor-owned adapter.
pub struct NativeServices {
    policy: Arc<AccessPolicy>,
    editor: Arc<dyn HostServices>,
    jobs: Arc<Semaphore>,
    editor_jobs: Arc<Semaphore>,
    io: Arc<Semaphore>,
    storage: Option<Arc<PrivateStorage>>,
    storage_lock: Arc<tokio::sync::Mutex<()>>,
}

struct PrivateStorage {
    path: PathBuf,
    directory: Mutex<Option<Arc<ScopedDirectory>>>,
}

impl PrivateStorage {
    /// Called only inside a blocking I/O task with the shared I/O permit held.
    fn open(&self) -> Result<Arc<ScopedDirectory>, ServiceError> {
        let mut cached = self.directory.lock();
        if let Some(directory) = cached.as_ref() {
            return Ok(directory.clone());
        }
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&self.path).map_err(host_failure)?;
        let directory = Arc::new(ScopedDirectory::open(&self.path).map_err(host_failure)?);
        *cached = Some(directory.clone());
        Ok(directory)
    }
}

impl NativeServices {
    /// Bind one invocation's editor origin while retaining generation-owned
    /// policy, storage locks, and native admission across queued invocations.
    pub fn with_editor(&self, editor: Arc<dyn HostServices>) -> Self {
        Self {
            policy: self.policy.clone(),
            editor,
            jobs: self.jobs.clone(),
            editor_jobs: self.editor_jobs.clone(),
            io: self.io.clone(),
            storage: self.storage.clone(),
            storage_lock: self.storage_lock.clone(),
        }
    }

    pub fn new(
        policy: Arc<AccessPolicy>,
        editor: Arc<dyn HostServices>,
        storage: Option<&Path>,
    ) -> Result<Self, ServiceError> {
        Self::with_budget(policy, editor, storage, NativeBudget::default())
    }

    pub fn with_budget(
        policy: Arc<AccessPolicy>,
        editor: Arc<dyn HostServices>,
        storage: Option<&Path>,
        budget: NativeBudget,
    ) -> Result<Self, ServiceError> {
        let storage = if policy.require(Capability::Storage).is_ok() {
            storage.map(|path| {
                Arc::new(PrivateStorage {
                    path: path.to_owned(),
                    directory: Mutex::new(None),
                })
            })
        } else {
            None
        };
        Ok(Self {
            policy,
            editor,
            jobs: Arc::new(Semaphore::new(MAX_JOBS)),
            editor_jobs: budget.jobs,
            io: budget.io,
            storage,
            storage_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    fn storage(&self, key: &str) -> Result<(Arc<PrivateStorage>, String), ServiceError> {
        self.policy.require(Capability::Storage)?;
        if key.is_empty() || key.len() > 256 {
            return Err(failure(
                ErrorCode::InvalidRequest,
                "storage key must contain 1 to 256 bytes",
            ));
        }
        let directory = self.storage.clone().ok_or_else(|| {
            failure(
                ErrorCode::UnsupportedInterface,
                "private plugin storage is unavailable",
            )
        })?;
        let filename = Sha256::digest(key.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok((directory, filename))
    }
}

impl HostServices for NativeServices {
    fn notify_job_ready(&self, job: u64) -> HostFuture<()> {
        match self.policy.check_live() {
            Ok(()) => self.editor.notify_job_ready(job),
            Err(error) => Box::pin(async { Err(error) }),
        }
    }
    fn editor_request(
        &self,
        request: plugin_api::editor::EditorRequest,
    ) -> HostFuture<plugin_api::editor::EditorReply> {
        let authorized = (|| {
            request.validate()?;
            for capability in request.capabilities() {
                self.policy.require(capability)?;
            }
            Ok::<_, ServiceError>(())
        })();
        match authorized {
            Ok(()) => self.editor.editor_request(request),
            Err(error) => Box::pin(async { Err(error) }),
        }
    }
    fn read_document(&self, request: ReadRequest) -> HostFuture<String> {
        match self.policy.require(Capability::EditorRead) {
            Ok(()) => self.editor.read_document(request),
            Err(error) => Box::pin(async { Err(error) }),
        }
    }

    fn read_roots(&self) -> HostFuture<Vec<ReadRoot>> {
        let policy = self.policy.clone();
        Box::pin(async move { policy.read_roots() })
    }

    fn read_file(&self, root: u32, path: String) -> HostFuture<String> {
        let policy = self.policy.clone();
        let io = self.io.clone();
        Box::pin(async move {
            if path.len() > 4096 {
                return Err(failure(
                    ErrorCode::ResourceExhausted,
                    "file path exceeds 4096 bytes",
                ));
            }
            policy.require(Capability::WorkspaceRead)?;
            let permit = io.acquire_owned().await.map_err(host_failure)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let bytes = policy
                    .read_root(root)?
                    .read(Path::new(&path))
                    .map_err(host_failure)?;
                policy.check_live()?;
                String::from_utf8(bytes).map_err(|_| {
                    failure(
                        ErrorCode::InvalidRequest,
                        "workspace read requires UTF-8 text",
                    )
                })
            })
            .await
            .map_err(host_failure)?
        })
    }

    fn write_file(&self, root: u32, path: String, value: String) -> HostFuture<()> {
        let policy = self.policy.clone();
        let io = self.io.clone();
        Box::pin(async move {
            if path.len() > 4096 || value.len() > crate::filesystem::MAX_FILE_BYTES {
                return Err(failure(
                    ErrorCode::ResourceExhausted,
                    "file path or contents exceed their byte limit",
                ));
            }
            policy.require(Capability::WorkspaceWrite)?;
            let permit = io.acquire_owned().await.map_err(host_failure)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                policy.write_root(root, Path::new(&path), value.as_bytes())
            })
            .await
            .map_err(host_failure)?
        })
    }

    fn start_job(&self, request: JobRequest) -> HostFuture<Arc<dyn HostJob>> {
        let prepare = (|| {
            self.policy.check_live()?;
            match &request {
                JobRequest::Timer { milliseconds } if *milliseconds <= 60_000 => {}
                JobRequest::Timer { .. } => {
                    return Err(failure(
                        ErrorCode::InvalidRequest,
                        "timer exceeds 60 seconds",
                    ))
                }
                JobRequest::Search { root, query } => {
                    self.policy.read_root(*root)?;
                    if query.is_empty() || query.len() > 4096 {
                        return Err(failure(
                            ErrorCode::InvalidRequest,
                            "search query must contain 1 to 4096 bytes",
                        ));
                    }
                }
                JobRequest::Process {
                    command,
                    args,
                    input,
                    root,
                    timeout_milliseconds,
                } => {
                    self.policy.process_root(*root, command, args)?;
                    JobRequest::validate_process_timeout(*timeout_milliseconds)?;
                    if args.len() > 64
                        || args.iter().map(String::len).sum::<usize>() > 64 * 1024
                        || input.len() > MAX_OUTPUT
                    {
                        return Err(failure(
                            ErrorCode::ResourceExhausted,
                            "process arguments or input exceed the limit",
                        ));
                    }
                    #[cfg(not(unix))]
                    return Err(failure(
                        ErrorCode::UnsupportedInterface,
                        "native process jobs require process-tree cleanup on this platform",
                    ));
                }
            }
            let permit =
                self.jobs.clone().try_acquire_owned().map_err(|_| {
                    failure(ErrorCode::ResourceExhausted, "plugin job limit reached")
                })?;
            let editor_permit = self.editor_jobs.clone().try_acquire_owned().map_err(|_| {
                failure(
                    ErrorCode::ResourceExhausted,
                    "editor native job limit reached",
                )
            })?;
            Ok((permit, editor_permit))
        })();
        let policy = self.policy.clone();
        let io = self.io.clone();
        Box::pin(async move {
            let permits = prepare?;
            let admission = Arc::new(JobAdmission { _permits: permits });
            let task_admission = admission.clone();
            let cancel = CancellationToken::new();
            let token = cancel.clone();
            let (sender, receiver) = mpsc::channel(8);
            let (progress, _) = watch::channel(0_u64);
            let progress = Arc::new(progress);
            let task_progress = progress.clone();
            // The returned resource owns the task from the moment it is spawned.
            // No await occurs between spawning and returning that ownership.
            let task = tokio::spawn(async move {
                let _admission = task_admission;
                let operation = async {
                    match request {
                        JobRequest::Timer { milliseconds } => {
                            tokio::select! { _ = token.cancelled() => Err(cancelled()), _ = tokio::time::sleep(Duration::from_millis(milliseconds)) => {
                                sender.send(Ok(JobOutput::Timer)).await.map_err(|_| cancelled())?;
                                task_progress.send_modify(|sequence| *sequence = sequence.saturating_add(1));
                                Ok(())
                            }}
                        }
                        JobRequest::Search { root, query } => {
                            let _io = tokio::select! { _ = token.cancelled() => return Err(cancelled()), permit = io.acquire_owned() => permit.map_err(host_failure)? };
                            let scan_token = token.clone();
                            let stream = sender.clone();
                            let scan_progress = task_progress.clone();
                            tokio::task::spawn_blocking(move || {
                                search(&policy, root, &query, &scan_token, &stream, &scan_progress)
                            })
                            .await
                            .map_err(host_failure)?
                        }
                        request @ JobRequest::Process { .. } => {
                            let output = process(&policy, request, &token).await?;
                            sender.send(Ok(output)).await.map_err(|_| cancelled())?;
                            task_progress
                                .send_modify(|sequence| *sequence = sequence.saturating_add(1));
                            Ok(())
                        }
                    }
                };
                let result = operation.await;
                if let Err(error) = result {
                    let _ = sender.try_send(Err(error));
                }
                drop(sender);
                task_progress.send_modify(|sequence| *sequence = sequence.saturating_add(1));
            });
            Ok(Arc::new(NativeJob {
                inner: Arc::new(JobInner {
                    cancel,
                    task: tokio::sync::Mutex::new(Some(task)),
                    receiver: Mutex::new(receiver),
                    progress,
                    _admission: admission,
                }),
            }) as Arc<dyn HostJob>)
        })
    }

    fn storage_read(&self, key: String) -> HostFuture<Option<String>> {
        let selected = self.storage(&key);
        let policy = self.policy.clone();
        let io = self.io.clone();
        let lock = self.storage_lock.clone();
        Box::pin(async move {
            let (directory, key) = selected?;
            let guard = lock.lock_owned().await;
            let permit = io.acquire_owned().await.map_err(host_failure)?;
            tokio::task::spawn_blocking(move || {
                let (_guard, _permit) = (guard, permit);
                policy.require(Capability::Storage)?;
                let directory = directory.open()?;
                match directory.directory().symlink_metadata(&key) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(host_failure(error)),
                    Ok(_) => {
                        let bytes = directory
                            .read_bounded(Path::new(&key), MAX_STORAGE_VALUE)
                            .map_err(host_failure)?;
                        policy.check_live()?;
                        String::from_utf8(bytes).map(Some).map_err(host_failure)
                    }
                }
            })
            .await
            .map_err(host_failure)?
        })
    }

    fn storage_write(&self, key: String, value: String) -> HostFuture<()> {
        let selected = self.storage(&key);
        let policy = self.policy.clone();
        let io = self.io.clone();
        let lock = self.storage_lock.clone();
        Box::pin(async move {
            if value.len() > MAX_STORAGE_VALUE {
                return Err(failure(
                    ErrorCode::ResourceExhausted,
                    "storage value exceeds 64 KiB",
                ));
            }
            let (directory, key) = selected?;
            let guard = lock.lock_owned().await;
            let permit = io.acquire_owned().await.map_err(host_failure)?;
            tokio::task::spawn_blocking(move || {
                let (_guard, _permit) = (guard, permit);
                policy.require(Capability::Storage)?;
                let directory = directory.open()?;
                let mut bytes = 0;
                let mut keys = 0;
                for entry in directory.directory().entries().map_err(host_failure)? {
                    let entry = entry.map_err(host_failure)?;
                    if entry.file_name() == key.as_str() {
                        continue;
                    }
                    let metadata = entry.metadata().map_err(host_failure)?;
                    keys += 1;
                    bytes += metadata.len();
                    if keys >= MAX_STORAGE_KEYS || bytes + value.len() as u64 > MAX_STORAGE_BYTES {
                        return Err(failure(
                            ErrorCode::ResourceExhausted,
                            "private plugin storage quota reached",
                        ));
                    }
                }
                policy.check_live()?;
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let temporary = format!(
                    "tmp-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                );
                let mut options = cap_std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use cap_std::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = directory
                    .directory()
                    .open_with(&temporary, &options)
                    .map_err(host_failure)?;
                let result = (|| {
                    std::io::Write::write_all(&mut file, value.as_bytes()).map_err(host_failure)?;
                    file.sync_all().map_err(host_failure)?;
                    policy.check_live()?;
                    directory
                        .directory()
                        .rename(&temporary, directory.directory(), &key)
                        .map_err(host_failure)
                })();
                if result.is_err() {
                    let _ = directory.directory().remove_file(&temporary);
                }
                result
            })
            .await
            .map_err(host_failure)?
        })
    }
}

struct JobInner {
    cancel: CancellationToken,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    receiver: Mutex<mpsc::Receiver<Result<JobOutput, ServiceError>>>,
    progress: Arc<watch::Sender<u64>>,
    // Completed jobs can still retain output and a readiness notification.
    // Keep admission charged until the resource itself is released.
    _admission: Arc<JobAdmission>,
}
struct JobAdmission {
    _permits: (
        tokio::sync::OwnedSemaphorePermit,
        tokio::sync::OwnedSemaphorePermit,
    ),
}
impl Drop for JobInner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
struct NativeJob {
    inner: Arc<JobInner>,
}
impl HostJob for NativeJob {
    fn ready(&self, after: u64) -> HostFuture<u64> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let mut progress = inner.progress.subscribe();
            loop {
                let sequence = *progress.borrow_and_update();
                if sequence > after {
                    return Ok(sequence);
                }
                tokio::select! {
                    _ = inner.cancel.cancelled() => return Err(cancelled()),
                    changed = progress.changed() => changed.map_err(host_failure)?,
                }
            }
        })
    }
    fn poll(&self) -> HostFuture<JobPoll> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let result = match inner.receiver.lock().try_recv() {
                Ok(Ok(output)) => Ok(JobPoll::Ready { output }),
                Ok(Err(error)) => Ok(JobPoll::Failed { error }),
                Err(mpsc::error::TryRecvError::Empty) => Ok(JobPoll::Pending),
                Err(mpsc::error::TryRecvError::Disconnected) => Ok(JobPoll::Finished),
            };
            if result == Ok(JobPoll::Pending) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            result
        })
    }
    fn cancel(&self) -> HostFuture<()> {
        let inner = self.inner.clone();
        Box::pin(async move {
            inner.cancel.cancel();
            if let Some(task) = inner.task.lock().await.take() {
                task.await.map_err(host_failure)?;
            }
            Ok(())
        })
    }
}

fn search(
    policy: &AccessPolicy,
    root: u32,
    query: &str,
    cancel: &CancellationToken,
    sender: &mpsc::Sender<Result<JobOutput, ServiceError>>,
    progress: &watch::Sender<u64>,
) -> Result<(), ServiceError> {
    let directory = policy.read_root(root)?.directory();
    let start = Instant::now();
    let mut pending = VecDeque::from([PathBuf::new()]);
    let mut scanned = 0;
    let mut bytes = 0;
    let mut matches = Vec::new();
    let mut delivered = 0;
    while let Some(path) = pending.pop_front() {
        policy.check_live()?;
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        if start.elapsed() > JOB_DEADLINE {
            return Err(failure(
                ErrorCode::DeadlineExceeded,
                "plugin search exceeded its deadline",
            ));
        }
        let dir = if path.as_os_str().is_empty() {
            directory.try_clone()
        } else {
            directory.open_dir(&path)
        }
        .map_err(host_failure)?;
        for entry in dir.entries().map_err(host_failure)? {
            if cancel.is_cancelled() {
                return Err(cancelled());
            }
            let entry = entry.map_err(host_failure)?;
            scanned += 1;
            if scanned > 4096 || bytes > 64 * 1024 * 1024 || delivered + matches.len() >= 512 {
                send_search(
                    sender,
                    cancel,
                    progress,
                    JobOutput::Search {
                        matches,
                        truncated: true,
                    },
                )?;
                return Ok(());
            }
            let name = entry.file_name();
            let child = path.join(&name);
            let kind = entry.file_type().map_err(host_failure)?;
            if kind.is_dir() {
                if !name.to_string_lossy().starts_with('.') && child.components().count() <= 16 {
                    pending.push_back(child);
                }
            } else if kind.is_file() {
                let Ok(content) = policy.read_root(root)?.read(&child) else {
                    continue;
                };
                bytes += content.len();
                let Ok(text) = std::str::from_utf8(&content) else {
                    continue;
                };
                for (line, text) in text.lines().enumerate() {
                    if line % 256 == 0 {
                        policy.check_live()?;
                        if cancel.is_cancelled() {
                            return Err(cancelled());
                        }
                        if start.elapsed() > JOB_DEADLINE {
                            return Err(failure(
                                ErrorCode::DeadlineExceeded,
                                "plugin search exceeded its deadline",
                            ));
                        }
                    }
                    if let Some(column) = text.find(query) {
                        let excerpt: String = text.chars().take(1024).collect();
                        matches.push(SearchMatch {
                            path: child.to_string_lossy().into_owned(),
                            line: line as u64,
                            column: text[..column].chars().count() as u64,
                            text: excerpt,
                        });
                        if matches.len() == 64 {
                            delivered += matches.len();
                            send_search(
                                sender,
                                cancel,
                                progress,
                                JobOutput::Search {
                                    matches: std::mem::take(&mut matches),
                                    truncated: false,
                                },
                            )?;
                        }
                        if delivered >= 512 {
                            send_search(
                                sender,
                                cancel,
                                progress,
                                JobOutput::Search {
                                    matches,
                                    truncated: true,
                                },
                            )?;
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
    send_search(
        sender,
        cancel,
        progress,
        JobOutput::Search {
            matches,
            truncated: false,
        },
    )
}

fn send_search(
    sender: &mpsc::Sender<Result<JobOutput, ServiceError>>,
    cancel: &CancellationToken,
    progress: &watch::Sender<u64>,
    output: JobOutput,
) -> Result<(), ServiceError> {
    let mut output = Ok(output);
    let start = Instant::now();
    loop {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        if start.elapsed() > JOB_DEADLINE {
            return Err(failure(
                ErrorCode::DeadlineExceeded,
                "plugin search consumer did not drain results",
            ));
        }
        match sender.try_send(output) {
            Ok(()) => {
                progress.send_modify(|sequence| *sequence = sequence.saturating_add(1));
                return Ok(());
            }
            Err(mpsc::error::TrySendError::Closed(_)) => return Err(cancelled()),
            Err(mpsc::error::TrySendError::Full(value)) => {
                output = value;
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

#[cfg(unix)]
struct ChildProcess(tokio::process::Child);
#[cfg(unix)]
impl ChildProcess {
    fn terminate(&mut self) {
        if let Some(id) = self.0.id() {
            // The child is unreaped while it has an id, preventing PID reuse.
            unsafe {
                libc::kill(-(id as i32), libc::SIGKILL);
            }
            let _ = self.0.start_kill();
        }
    }
}
#[cfg(unix)]
impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(unix)]
async fn process(
    policy: &AccessPolicy,
    request: JobRequest,
    cancel: &CancellationToken,
) -> Result<JobOutput, ServiceError> {
    use std::process::Stdio;
    let JobRequest::Process {
        command: executable,
        args,
        input,
        root,
        timeout_milliseconds,
    } = request
    else {
        unreachable!()
    };
    JobRequest::validate_process_timeout(timeout_milliseconds)?;
    let cwd = policy.process_root(root, &executable, &args)?;
    let mut command = tokio::process::Command::new(executable);
    command
        .args(&args)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    if policy.require(Capability::Environment).is_ok() {
        for name in &policy.permissions.environment {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }
    let mut child = ChildProcess(command.spawn().map_err(host_failure)?);
    let mut stdin = child.0.stdin.take().unwrap();
    let stdout = child.0.stdout.take().unwrap();
    let stderr = child.0.stderr.take().unwrap();
    let operation = async {
        let write = async move {
            stdin.write_all(input.as_bytes()).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<(), std::io::Error>(())
        };
        let read = |pipe: Box<dyn tokio::io::AsyncRead + Unpin + Send>| async move {
            let mut bytes = Vec::new();
            pipe.take((MAX_OUTPUT + 1) as u64)
                .read_to_end(&mut bytes)
                .await
                .map_err(host_failure)?;
            if bytes.len() > MAX_OUTPUT {
                return Err(failure(
                    ErrorCode::ResourceExhausted,
                    "native tool output exceeds 1 MiB per stream",
                ));
            }
            String::from_utf8(bytes).map_err(|_| {
                failure(
                    ErrorCode::InvalidRequest,
                    "native tool output is not UTF-8 text",
                )
            })
        };
        // Keep the leader unreaped until pipes close. A descendant retaining a
        // pipe can then be killed by group on timeout without a reused PID race.
        let (stdout, stderr, ()) =
            tokio::try_join!(read(Box::new(stdout)), read(Box::new(stderr)), async {
                write.await.map_err(host_failure)
            })?;
        let status = child.0.wait().await.map_err(host_failure)?;
        policy.check_live()?;
        Ok(JobOutput::Process {
            status: status.code().unwrap_or(-1),
            stdout,
            stderr,
        })
    };
    let result = tokio::select! {
        _ = cancel.cancelled() => Err(cancelled()),
        result = tokio::time::timeout(Duration::from_millis(timeout_milliseconds), operation) => result.unwrap_or_else(|_| Err(failure(ErrorCode::DeadlineExceeded, "native tool exceeded its process timeout"))),
    };
    if result.is_err() {
        child.terminate();
        let _ = child.0.wait().await;
    }
    result
}

#[cfg(not(unix))]
async fn process(
    _policy: &AccessPolicy,
    _request: JobRequest,
    _cancel: &CancellationToken,
) -> Result<JobOutput, ServiceError> {
    Err(failure(
        ErrorCode::UnsupportedInterface,
        "native process jobs are unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_api::{CapabilitySet, Permissions, ProcessGrant};

    struct NoEditor;
    impl HostServices for NoEditor {
        fn read_document(&self, _: ReadRequest) -> HostFuture<String> {
            Box::pin(async { Err(failure(ErrorCode::UnsupportedInterface, "no editor")) })
        }
    }

    fn services(
        root: &Path,
        capabilities: CapabilitySet,
        processes: Vec<ProcessGrant>,
    ) -> NativeServices {
        let policy = AccessPolicy::new(
            capabilities.clone(),
            Permissions {
                capabilities,
                read_roots: vec![root.into()],
                processes,
                ..Permissions::default()
            },
            Path::new("."),
        )
        .unwrap();
        NativeServices::new(
            Arc::new(policy),
            Arc::new(NoEditor),
            Some(&root.join("storage")),
        )
        .unwrap()
    }

    async fn next(job: &Arc<dyn HostJob>) -> JobPoll {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let poll = job.poll().await.unwrap();
                if poll != JobPoll::Pending {
                    return poll;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn read_root_metadata_requires_grants_and_rechecks_revocation_when_polled() {
        let root = tempfile::tempdir().unwrap();
        let denied = services(root.path(), Default::default(), vec![]);
        assert_eq!(
            denied.read_roots().await.unwrap_err().code,
            ErrorCode::PermissionDenied
        );
        let host = services(root.path(), [Capability::WorkspaceRead].into(), vec![]);
        assert_eq!(
            host.read_roots().await.unwrap(),
            vec![ReadRoot {
                index: 0,
                path: root.path().canonicalize().unwrap().to_str().unwrap().into(),
                configured_path: root.path().to_str().unwrap().into(),
            }]
        );
        let pending = host.read_roots();
        host.policy.revoke();
        assert_eq!(pending.await.unwrap_err().code, ErrorCode::Cancelled);
    }

    #[tokio::test]
    async fn search_streams_unicode_locations_and_cancels_a_full_result_queue() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("unicode.txt"), "é前 needle\n").unwrap();
        let host = services(root.path(), [Capability::WorkspaceRead].into(), vec![]);
        let job = host
            .start_job(JobRequest::Search {
                root: 0,
                query: "needle".into(),
            })
            .await
            .unwrap();
        let JobPoll::Ready {
            output: JobOutput::Search { matches, truncated },
        } = next(&job).await
        else {
            panic!("expected search results")
        };
        assert!(!truncated);
        assert_eq!(matches[0].column, 3);
        job.cancel().await.unwrap();
        std::fs::write(root.path().join("many.txt"), "needle\n".repeat(1000)).unwrap();
        let job = host
            .start_job(JobRequest::Search {
                root: 0,
                query: "needle".into(),
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        tokio::time::timeout(Duration::from_secs(1), job.cancel())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn job_readiness_waits_for_progress_and_cancellation() {
        let root = tempfile::tempdir().unwrap();
        let host = services(root.path(), Default::default(), vec![]);
        let job = host
            .start_job(JobRequest::Timer { milliseconds: 5 })
            .await
            .unwrap();
        let sequence = tokio::time::timeout(Duration::from_secs(1), job.ready(0))
            .await
            .unwrap()
            .unwrap();
        assert!(sequence > 0);
        assert_eq!(
            job.poll().await.unwrap(),
            JobPoll::Ready {
                output: JobOutput::Timer
            }
        );
        while job.poll().await.unwrap() != JobPoll::Finished {
            tokio::task::yield_now().await;
        }
        let latest = job.ready(0).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), job.ready(latest))
                .await
                .is_err()
        );
        job.cancel().await.unwrap();
        assert_eq!(
            job.ready(latest).await.unwrap_err().code,
            ErrorCode::Cancelled
        );
    }

    #[tokio::test]
    async fn private_storage_is_durable_bounded_and_revoked() {
        let root = tempfile::tempdir().unwrap();
        let host = services(root.path(), [Capability::Storage].into(), vec![]);
        assert!(!root.path().join("storage").exists());
        assert_eq!(host.storage_read("recent".into()).await.unwrap(), None);
        host.storage_write("recent".into(), "é文件".into())
            .await
            .unwrap();
        let reopened = services(root.path(), [Capability::Storage].into(), vec![]);
        assert_eq!(
            reopened
                .storage_read("recent".into())
                .await
                .unwrap()
                .as_deref(),
            Some("é文件")
        );
        assert_eq!(
            host.storage_write("large".into(), "x".repeat(MAX_STORAGE_VALUE + 1))
                .await
                .unwrap_err()
                .code,
            ErrorCode::ResourceExhausted
        );
        host.policy.revoke();
        assert_eq!(
            host.storage_read("recent".into()).await.unwrap_err().code,
            ErrorCode::Cancelled
        );
    }

    #[tokio::test]
    async fn completed_job_resources_still_reserve_editor_admission() {
        let budget = NativeBudget::default();
        let mut hosts = Vec::new();
        for _ in 0..3 {
            let policy = Arc::new(
                AccessPolicy::new(Default::default(), Permissions::default(), Path::new("."))
                    .unwrap(),
            );
            hosts.push(
                NativeServices::with_budget(policy, Arc::new(NoEditor), None, budget.clone())
                    .unwrap(),
            );
        }
        let mut jobs = Vec::new();
        for host in &hosts[..2] {
            for _ in 0..MAX_JOBS {
                let job = host
                    .start_job(JobRequest::Timer { milliseconds: 0 })
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(1), job.ready(0))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(matches!(job.poll().await.unwrap(), JobPoll::Ready { .. }));
                while job.poll().await.unwrap() != JobPoll::Finished {
                    tokio::task::yield_now().await;
                }
                jobs.push(job);
            }
        }
        assert!(matches!(
            hosts[2]
                .start_job(JobRequest::Timer { milliseconds: 0 })
                .await,
            Err(ServiceError {
                code: ErrorCode::ResourceExhausted,
                ..
            })
        ));
        drop(jobs);
        let job = hosts[2]
            .start_job(JobRequest::Timer { milliseconds: 0 })
            .await
            .unwrap();
        job.cancel().await.unwrap();
    }

    #[tokio::test]
    async fn native_job_budget_is_shared_across_packages_and_released_after_cleanup() {
        let budget = NativeBudget::default();
        let mut hosts = Vec::new();
        for _ in 0..3 {
            let policy = AccessPolicy::new(
                [Capability::Ui].into(),
                Permissions::default(),
                Path::new("."),
            )
            .unwrap();
            hosts.push(
                NativeServices::with_budget(
                    Arc::new(policy),
                    Arc::new(NoEditor),
                    None,
                    budget.clone(),
                )
                .unwrap(),
            );
        }
        let mut jobs = Vec::new();
        for host in &hosts[..2] {
            for _ in 0..MAX_JOBS {
                jobs.push(
                    host.start_job(JobRequest::Timer {
                        milliseconds: 60_000,
                    })
                    .await
                    .unwrap(),
                );
            }
        }
        assert!(matches!(
            hosts[2]
                .start_job(JobRequest::Timer { milliseconds: 0 })
                .await,
            Err(ServiceError {
                code: ErrorCode::ResourceExhausted,
                ..
            })
        ));
        for job in jobs {
            job.cancel().await.unwrap();
        }
        let job = hosts[2]
            .start_job(JobRequest::Timer { milliseconds: 0 })
            .await
            .unwrap();
        job.cancel().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_timeout_is_bounded_and_deadline_or_cancellation_stops_work() {
        let root = tempfile::tempdir().unwrap();
        let host = services(
            root.path(),
            [Capability::Process].into(),
            vec![ProcessGrant {
                command: "/bin/sh".into(),
                args: vec!["-c".into(), "**".into()],
            }],
        );
        let request = |timeout_milliseconds, script: &str| JobRequest::Process {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            input: String::new(),
            root: 0,
            timeout_milliseconds,
        };
        for milliseconds in [
            0,
            JobRequest::MAX_PROCESS_TIMEOUT_MILLISECONDS + 1,
            u64::MAX,
        ] {
            assert!(matches!(
                host.start_job(request(milliseconds, "exit 0")).await,
                Err(ServiceError {
                    code: ErrorCode::InvalidRequest,
                    ..
                })
            ));
        }
        let job = host
            .start_job(request(40, "/bin/sleep 0.3; printf leaked > expired"))
            .await
            .unwrap();
        assert!(matches!(
            next(&job).await,
            JobPoll::Failed {
                error: ServiceError {
                    code: ErrorCode::DeadlineExceeded,
                    ..
                }
            }
        ));
        job.cancel().await.unwrap();
        drop(job);

        let job = host
            .start_job(request(
                JobRequest::MAX_PROCESS_TIMEOUT_MILLISECONDS,
                "printf started > started; /bin/sleep 0.3; printf leaked > cancelled",
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !root.path().join("started").exists() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), job.cancel())
            .await
            .unwrap()
            .unwrap();
        drop(job);
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(!root.path().join("expired").exists());
        assert!(!root.path().join("cancelled").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_checks_arguments_bounds_output_and_cleans_up_descendant_pipes() {
        let root = tempfile::tempdir().unwrap();
        let script = "cat";
        let host = services(
            root.path(),
            [Capability::Process].into(),
            vec![ProcessGrant {
                command: "/bin/sh".into(),
                args: vec!["-c".into(), "**".into()],
            }],
        );
        let request = |command: &str, script: &str| JobRequest::Process {
            command: command.into(),
            args: vec!["-c".into(), script.into()],
            input: "input".into(),
            root: 0,
            timeout_milliseconds: JobRequest::DEFAULT_PROCESS_TIMEOUT_MILLISECONDS,
        };
        assert!(matches!(
            host.start_job(request("/bin/bash", script)).await,
            Err(ServiceError {
                code: ErrorCode::PermissionDenied,
                ..
            })
        ));
        let job = host.start_job(request("/bin/sh", script)).await.unwrap();
        let JobPoll::Ready {
            output: JobOutput::Process { status, stdout, .. },
        } = next(&job).await
        else {
            panic!("expected tool output")
        };
        assert_eq!((status, stdout.as_str()), (0, "input"));
        job.cancel().await.unwrap();
        let job = host.start_job(request("/bin/sh", "yes x")).await.unwrap();
        assert!(matches!(
            next(&job).await,
            JobPoll::Failed {
                error: ServiceError {
                    code: ErrorCode::ResourceExhausted,
                    ..
                }
            }
        ));
        job.cancel().await.unwrap();
        // The leader exits while its descendant holds stdout open. Cancellation
        // must still kill the group and finish cleanup promptly.
        let job = host
            .start_job(request("/bin/sh", "sleep 30 & exit 0"))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        tokio::time::timeout(Duration::from_secs(1), job.cancel())
            .await
            .unwrap()
            .unwrap();
    }
}

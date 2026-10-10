use super::*;
use plugin_api::{
    Action, Capability, DocumentSnapshot, EditorContext, HostFuture, HostJob, JobPoll, JobRequest,
    ReadRequest, SelectionRange, ViewSnapshot,
};
use std::sync::atomic::AtomicUsize;

fn fixture() -> Arc<[u8]> {
    wit_component::ComponentEncoder::default()
        .module(include_bytes!("../tests/fixtures/component-guest.wasm"))
        .unwrap()
        .validate(true)
        .encode()
        .unwrap()
        .into()
}
fn c_fixture() -> Arc<[u8]> {
    wit_component::ComponentEncoder::default()
        .module(include_bytes!("../tests/fixtures/c-guest.wasm"))
        .unwrap()
        .validate(true)
        .encode()
        .unwrap()
        .into()
}
fn sdk_fixture() -> Arc<[u8]> {
    wit_component::ComponentEncoder::default()
        .module(include_bytes!("../tests/fixtures/sdk-guest.wasm"))
        .unwrap()
        .validate(true)
        .encode()
        .unwrap()
        .into()
}
fn request(command: &str) -> Request {
    Request {
        abi_version: 3,
        event: Event::Command,
        command: Some(command.into()),
        args: Vec::new(),
        config: serde_json::Value::Null,
        data: serde_json::Value::Null,
        editor: EditorContext {
            generation: 1,
            mode: "normal".into(),
            document: Some(DocumentSnapshot {
                id: 1,
                version: 7,
                path: None,
                language: None,
                char_count: 6,
                byte_count: 7,
            }),
            view: Some(ViewSnapshot {
                id: 1,
                document: 1,
                binding_revision: 9,
                selection_revision: 4,
                selections: vec![SelectionRange { anchor: 0, head: 6 }],
                primary: 0,
            }),
        },
    }
}

#[cfg(target_os = "macos")]
#[test]
fn macos_unix_traps_recover_guest_faults_and_forward_native_signals() {
    const PROBE: &str = "MITOS_PLUGIN_NATIVE_TRAP_PROBE";
    if std::env::var_os(PROBE).is_some() {
        extern "C" fn native_signal(_: libc::c_int) {
            unsafe { libc::_exit(42) }
        }
        // Install the prior native handler in the isolated process. The guest
        // fault must remain a Wasmtime trap; a later native signal reaches this
        // handler while the same engine/store is still alive.
        unsafe {
            assert_ne!(
                libc::signal(
                    libc::SIGSEGV,
                    native_signal as *const () as libc::sighandler_t,
                ),
                libc::SIG_ERR
            );
        }
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                let engine = component::engine().unwrap();
                let bytes = wat::parse_str(
                    r#"(module (memory 1)
                        (func (export "read") (param i32) (result i32)
                            local.get 0 i32.load))"#,
                )
                .unwrap();
                let module = wasmtime::Module::from_binary(&engine, &bytes).unwrap();
                let mut store = wasmtime::Store::new(&engine, ());
                store.set_epoch_deadline(u64::MAX);
                let instance = wasmtime::Instance::new_async(&mut store, &module, &[])
                    .await
                    .unwrap();
                let read = instance
                    .get_typed_func::<i32, i32>(&mut store, "read")
                    .unwrap();
                let trap = read.call_async(&mut store, 65536).await.unwrap_err();
                assert_eq!(
                    trap.downcast_ref::<wasmtime::Trap>(),
                    Some(&wasmtime::Trap::MemoryOutOfBounds)
                );
                eprintln!("guest memory fault recovered");
                unsafe { libc::raise(libc::SIGSEGV) };
                panic!("native SIGSEGV was swallowed");
            });
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "worker::tests::macos_unix_traps_recover_guest_faults_and_forward_native_signals",
            "--nocapture",
        ])
        .env(PROBE, "1")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(42),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("guest memory fault recovered"));
}

#[test]
fn initialization_precedes_reserved_commands_and_notifications() {
    let envelope = |event| {
        let (result, _) = oneshot::channel();
        let mut request = request("status");
        request.event = event;
        Envelope {
            submitted: Instant::now(),
            config_json: None,
            target: InvocationTarget::default(),
            cancel: CancellationToken::new(),
            request,
            services: Arc::new(Services::default()),
            bytes: 1,
            result,
        }
    };
    let mut mailbox = Mailbox::default();
    mailbox
        .notifications
        .push_back(envelope(Event::DocumentOpened));
    mailbox.notifications.push_back(envelope(Event::Init));
    mailbox.commands.push_back(envelope(Event::Command));
    mailbox.bytes = 3;
    assert_eq!(mailbox.pop().unwrap().request.event, Event::Init);
    assert_eq!(mailbox.pop().unwrap().request.event, Event::Command);
    assert_eq!(mailbox.pop().unwrap().request.event, Event::DocumentOpened);
    assert_eq!(mailbox.bytes, 0);
}
#[derive(Default)]
struct Services {
    reads: Arc<AtomicUsize>,
    starts: Arc<AtomicUsize>,
    jobs: Arc<AtomicUsize>,
    creating: Arc<AtomicUsize>,
    read_delay: Duration,
    creation_delay: Duration,
    notifications: Arc<Mutex<Vec<u64>>>,
    job_ready: bool,
    reject_notification: bool,
}
impl HostServices for Services {
    fn read_document(&self, request: ReadRequest) -> HostFuture<String> {
        self.starts.fetch_add(1, Ordering::AcqRel);
        let reads = self.reads.clone();
        let delay = self.read_delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            if request.generation != 1
                || request.document != 1
                || request.version != 7
                || request.start != 0
                || request.end != 6
            {
                return Err(error(ErrorCode::StaleState, "test snapshot mismatch"));
            }
            reads.fetch_add(1, Ordering::AcqRel);
            Ok("straße".into())
        })
    }
    fn start_job(&self, _request: JobRequest) -> HostFuture<Arc<dyn HostJob>> {
        let jobs = self.jobs.clone();
        let creating = self.creating.clone();
        let delay = self.creation_delay;
        let job_ready = self.job_ready;
        Box::pin(async move {
            struct Creating(Arc<AtomicUsize>);
            impl Drop for Creating {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::AcqRel);
                }
            }
            creating.fetch_add(1, Ordering::AcqRel);
            let _creating = Creating(creating);
            tokio::time::sleep(delay).await;
            jobs.fetch_add(1, Ordering::AcqRel);
            struct Child(Arc<AtomicUsize>);
            impl Drop for Child {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::AcqRel);
                }
            }
            let child = Child(jobs.clone());
            let cancel = CancellationToken::new();
            let stopping = cancel.clone();
            let (ready, receiver) = tokio::sync::watch::channel(0);
            let task = tokio::spawn(async move {
                let _child = child;
                if job_ready {
                    tokio::select! { _ = stopping.cancelled() => return, _ = tokio::time::sleep(Duration::from_millis(20)) => {} }
                    let _ = ready.send(1);
                }
                stopping.cancelled().await;
            });
            Ok(Arc::new(Job {
                cancel,
                ready: receiver,
                task: Arc::new(Mutex::new(Some(task))),
            }) as Arc<dyn HostJob>)
        })
    }
    fn notify_job_ready(&self, job: u64) -> HostFuture<()> {
        let notifications = self.notifications.clone();
        let reject = self.reject_notification;
        Box::pin(async move {
            if reject {
                return Err(cancelled());
            }
            notifications.lock().unwrap().push(job);
            Ok(())
        })
    }
}
struct Job {
    cancel: CancellationToken,
    task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    ready: tokio::sync::watch::Receiver<u64>,
}
impl HostJob for Job {
    fn ready(&self, after: u64) -> HostFuture<u64> {
        let mut ready = self.ready.clone();
        let cancel = self.cancel.clone();
        Box::pin(async move {
            loop {
                let sequence = *ready.borrow_and_update();
                if sequence > after {
                    return Ok(sequence);
                }
                tokio::select! { _ = cancel.cancelled() => return Err(cancelled()), result = ready.changed() => result.map_err(|_| cancelled())? }
            }
        })
    }
    fn poll(&self) -> HostFuture<JobPoll> {
        Box::pin(async { Ok(JobPoll::Pending) })
    }
    fn cancel(&self) -> HostFuture<()> {
        self.cancel.cancel();
        let tasks = self.task.clone();
        Box::pin(async move {
            let task = tasks.lock().unwrap().take();
            if let Some(task) = task {
                let _ = task.await;
            }
            Ok(())
        })
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
fn capabilities() -> CapabilitySet {
    [
        Capability::Ui,
        Capability::EditorRead,
        Capability::EditorEdit,
        Capability::EditorSelection,
    ]
    .into_iter()
    .collect()
}
fn plugin_actor(pool: &WorkerPool) -> PluginActor {
    pool.spawn(fixture(), 1, capabilities(), capabilities())
        .unwrap()
}

#[tokio::test]
async fn diagnostics_measure_queue_execution_and_final_application() {
    let pool = WorkerPool::new().unwrap();
    let actor = plugin_actor(&pool);
    let services = Arc::new(Services {
        read_delay: Duration::from_millis(20),
        ..Default::default()
    });
    let first = actor
        .invoke(request("uppercase"), services.clone())
        .unwrap();
    let second = actor.invoke(request("status"), services).unwrap();
    let _first = first.await.unwrap();
    let _second = second.await.unwrap();
    let (queued, timings, entries) = actor.diagnostics();
    assert_eq!(queued, 0);
    assert_eq!(timings.completed, 2);
    assert!(timings.queue_max_us >= 10_000);
    assert!(timings.execution_max_us >= 10_000);
    assert!(entries
        .iter()
        .any(|entry| entry.message.starts_with("call ")));
    let stale = error(ErrorCode::StaleState, "host rejected stale response");
    actor.record_application(37, Some(&stale));
    let (_, timings, entries) = actor.diagnostics();
    assert_eq!((timings.completed, timings.failed), (1, 1));
    assert_eq!((timings.apply_last_us, timings.apply_max_us), (37, 37));
    assert_eq!(entries.last().unwrap().message, stale.message);
    actor.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[test]
fn diagnostic_ring_bounds_retention_and_utf8_messages() {
    let mut diagnostics = ActorDiagnostics::default();
    for _ in 0..130 {
        diagnostics.log(DiagnosticLevel::Error, &format!("\x1b{}", "μ".repeat(4096)));
    }
    assert_eq!(diagnostics.entries.len(), 128);
    assert_eq!(diagnostics.entries.front().unwrap().sequence, 3);
    assert_eq!(diagnostics.entries.back().unwrap().sequence, 130);
    assert!(diagnostics
        .entries
        .iter()
        .all(|entry| entry.message.len() == 2048
            && entry.message.chars().all(|character| character == 'μ')));
}

#[tokio::test]
async fn actual_rust_guest_preserves_state_and_reads_only_requested_unicode() {
    let pool = WorkerPool::new().unwrap();
    let actor = plugin_actor(&pool);
    let services = Arc::new(Services::default());
    for expected in ["call 1", "call 2"] {
        let response = actor
            .invoke(request("status"), services.clone())
            .unwrap()
            .await
            .unwrap();
        assert!(matches!(&response.actions[0], Action::Status { message } if message == expected));
    }
    let response = actor
        .invoke(request("uppercase"), services.clone())
        .unwrap()
        .await
        .unwrap();
    let Action::Edit {
        document,
        version,
        edits,
    } = &response.actions[0]
    else {
        panic!("missing staged edit")
    };
    assert_eq!((*document, *version), (1, 7));
    assert_eq!(edits[0].text, "STRASSE");
    assert_eq!((edits[0].start, edits[0].end), (0, 6));
    let Action::SetSelection {
        ranges,
        primary,
        view,
        binding_revision,
        selection_revision,
        ..
    } = &response.actions[1]
    else {
        panic!("missing projected selection")
    };
    assert_eq!((ranges[0].anchor, ranges[0].head, *primary), (0, 7, 0));
    assert_eq!((*view, *binding_revision, *selection_revision), (1, 9, 4));
    assert_eq!(services.reads.load(Ordering::Acquire), 1);
    actor.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn generated_c_bindings_share_the_world_and_release_effect_resources() {
    let pool = WorkerPool::new().unwrap();
    let services = Arc::new(Services::default());
    let actor = pool
        .spawn(c_fixture(), 1, capabilities(), capabilities())
        .unwrap();
    for _ in 0..100 {
        let response = actor
            .invoke(request("status"), services.clone())
            .unwrap()
            .await
            .unwrap();
        assert!(matches!(&response.actions[0], Action::Status { message } if message == "c-ready"));
    }
    let response = actor
        .invoke(request("read"), services.clone())
        .unwrap()
        .await
        .unwrap();
    assert!(matches!(&response.actions[0], Action::Status { message } if message == "straße"));
    let response = actor
        .invoke(request("edit"), services)
        .unwrap()
        .await
        .unwrap();
    assert!(
        matches!(&response.actions[0], Action::Edit { document: 1, version: 7, edits } if edits.len() == 1 && edits[0].text == "C")
    );
    actor.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn rust_sdk_adapter_streams_effects_and_uses_scoped_reads() {
    let pool = WorkerPool::new().unwrap();
    let services = Arc::new(Services::default());
    let actor = pool
        .spawn(sdk_fixture(), 1, capabilities(), capabilities())
        .unwrap();
    let response = actor
        .invoke(request("uppercase"), services.clone())
        .unwrap()
        .await
        .unwrap();
    assert!(
        matches!(&response.actions[0], Action::Edit { edits, .. } if edits.len() == 1 && edits[0].text == "STRASSE")
    );
    assert_eq!(services.reads.load(Ordering::Acquire), 1);
    drop(response);
    let response = actor
        .invoke(request("ui"), services)
        .unwrap()
        .await
        .unwrap();
    assert!(
        matches!(&response.actions[0], Action::ShowUi { kind: plugin_api::ui::UiKind::Prompt { initial, .. }, .. } if initial == "Initial")
    );
    assert!(
        matches!(&response.actions[1], Action::ShowUi { kind: plugin_api::ui::UiKind::Picker { rows, .. }, .. } if rows.len() == 1 && rows[0].preview.as_deref() == Some("Preview"))
    );
    actor.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn status_on_large_metadata_does_not_read_text_and_denials_keep_guest_healthy() {
    let pool = WorkerPool::new().unwrap();
    let grants = [Capability::Ui].into_iter().collect();
    let actor = pool.spawn(fixture(), 1, capabilities(), grants).unwrap();
    let services = Arc::new(Services::default());
    let mut context = request("status");
    context.editor.document.as_mut().unwrap().byte_count = 10 * 1024 * 1024;
    actor
        .invoke(context, services.clone())
        .unwrap()
        .await
        .unwrap();
    assert_eq!(services.reads.load(Ordering::Acquire), 0);
    let failure = actor
        .invoke(request("uppercase"), services.clone())
        .unwrap()
        .await
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::PermissionDenied);
    assert!(actor.is_active());
    actor
        .invoke(request("status"), services)
        .unwrap()
        .await
        .unwrap();
    actor.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_effect_batches_are_atomic_and_do_not_poison_other_plugins() {
    let pool = WorkerPool::new().unwrap();
    let services = Arc::new(Services::default());
    for (command, code) in [
        ("quota", ErrorCode::ResourceExhausted),
        ("large-string", ErrorCode::ResourceExhausted),
        ("unfinished", ErrorCode::InvalidRequest),
    ] {
        let actor = plugin_actor(&pool);
        let failure = actor
            .invoke(request(command), services.clone())
            .unwrap()
            .await
            .unwrap_err();
        assert_eq!(failure.code, code);
        assert!(actor.is_active());
        actor.shutdown().await.unwrap();
    }
    let broken = plugin_actor(&pool);
    let failure = broken
        .invoke(request("trap-after-finish"), services.clone())
        .unwrap()
        .await
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::GuestTrap);
    assert!(!broken.is_active());
    let healthy = plugin_actor(&pool);
    healthy
        .invoke(request("status"), services)
        .unwrap()
        .await
        .unwrap();
    healthy.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn loops_and_slow_reads_cancel_with_dedicated_epochs() {
    let pool = WorkerPool::new().unwrap();
    let services = Arc::new(Services {
        read_delay: Duration::from_secs(30),
        ..Services::default()
    });
    for command in ["loop", "uppercase"] {
        let actor = plugin_actor(&pool);
        let completion = actor.invoke(request(command), services.clone()).unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let start = Instant::now();
        actor.cancel_target(Some(1), None);
        let failure = tokio::time::timeout(Duration::from_millis(250), completion)
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(failure.code, ErrorCode::Cancelled);
        assert!(start.elapsed() < Duration::from_millis(250));
        actor.shutdown().await.unwrap();
    }
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn job_drop_and_actor_revocation_await_child_cleanup() {
    let pool = WorkerPool::new().unwrap();
    let services = Arc::new(Services::default());
    let actor = plugin_actor(&pool);
    actor
        .invoke(request("job-start"), services.clone())
        .unwrap()
        .await
        .unwrap();
    assert_eq!(services.jobs.load(Ordering::Acquire), 1);
    actor
        .invoke(request("job-poll"), services.clone())
        .unwrap()
        .await
        .unwrap();
    actor
        .invoke(request("job-cancel"), services.clone())
        .unwrap()
        .await
        .unwrap();
    assert_eq!(services.jobs.load(Ordering::Acquire), 0);
    actor
        .invoke(request("job-start"), services.clone())
        .unwrap()
        .await
        .unwrap();
    actor.shutdown().await.unwrap();
    assert_eq!(services.jobs.load(Ordering::Acquire), 0);
    let actor = plugin_actor(&pool);
    let failure = actor
        .invoke(request("job-quota"), services.clone())
        .unwrap()
        .await
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::ResourceExhausted);
    assert_eq!(services.jobs.load(Ordering::Acquire), 0);
    actor.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn memory_growth_traps_without_affecting_other_stores() {
    let pool = WorkerPool::new().unwrap();
    let services = Arc::new(Services::default());
    let broken = plugin_actor(&pool);
    let failure = broken
        .invoke(request("grow"), services.clone())
        .unwrap()
        .await
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::ResourceExhausted);
    assert!(!broken.is_active());
    assert_eq!(pool.inner.memory.load(Ordering::Acquire), 0);
    let healthy = plugin_actor(&pool);
    healthy
        .invoke(request("status"), services)
        .unwrap()
        .await
        .unwrap();
    healthy.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn admission_counts_unconsumed_results_and_rejects_old_generations() {
    let pool = WorkerPool::new().unwrap();
    let actor = plugin_actor(&pool);
    let services = Arc::new(Services::default());
    assert!(pool.inner.engine.lock().unwrap().is_none());
    let mut old = request("status");
    old.editor.generation = 0;
    assert_eq!(
        actor
            .invoke(old, services.clone())
            .unwrap()
            .await
            .unwrap_err()
            .code,
        ErrorCode::StaleState
    );
    let mut calls = Vec::new();
    for _ in 0..MAX_CALLS {
        calls.push(actor.invoke(request("status"), services.clone()).unwrap());
    }
    assert_eq!(
        actor
            .invoke(request("status"), services)
            .err()
            .unwrap()
            .code,
        ErrorCode::ResourceExhausted
    );
    for call in calls {
        call.await.unwrap();
    }
    actor.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn suspended_services_leave_other_plugins_schedulable() {
    let pool = WorkerPool::new().unwrap();
    let services = Arc::new(Services {
        read_delay: Duration::from_secs(30),
        ..Services::default()
    });
    let first = plugin_actor(&pool);
    let second = plugin_actor(&pool);
    let third = plugin_actor(&pool);
    let one = first
        .invoke(request("uppercase"), services.clone())
        .unwrap();
    let two = second
        .invoke(request("uppercase"), services.clone())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while services.starts.load(Ordering::Acquire) < 2 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(
        Duration::from_millis(250),
        third.invoke(request("status"), services).unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    first.cancel_target(Some(1), None);
    second.cancel_target(Some(1), None);
    assert_eq!(one.await.unwrap_err().code, ErrorCode::Cancelled);
    assert_eq!(two.await.unwrap_err().code, ErrorCode::Cancelled);
    first.shutdown().await.unwrap();
    second.shutdown().await.unwrap();
    third.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn preparation_classifies_import_and_export_mismatches_before_guest_startup() {
    let pool = WorkerPool::new().unwrap();
    for (source, expected) in [
        (
            r#"(component
                (type $host (instance
                    (type $call (func))
                    (export "future-call" (func (type $call)))))
                (import "mitos:plugin/host@9.0.0" (instance $host (type $host)))
                (alias export $host "future-call" (func $future))
                (core func $future (canon lower (func $future)))
                (core module $guest
                    (import "host" "future-call" (func $future))
                    (func $start unreachable) (start $start)
                    (func (export "handle") call $future))
                (core instance $host-core (export "future-call" (func $future)))
                (core instance $guest (instantiate $guest
                    (with "host" (instance $host-core))))
                (func (export "handle") (canon lift (core func $guest "handle"))))"#,
            "linking plugin component imports",
        ),
        (
            r#"(component
                (core module $trap (func $start unreachable) (start $start))
                (core instance (instantiate $trap)))"#,
            "checking plugin component exports",
        ),
        (
            r#"(component
                (core module $guest (func (export "handle")))
                (core instance $guest (instantiate $guest))
                (func (export "handle") (canon lift (core func $guest "handle"))))"#,
            "checking plugin component exports",
        ),
    ] {
        let bytes: Arc<[u8]> = wat::parse_str(source).unwrap().into();
        wasmparser::Validator::new().validate_all(&bytes).unwrap();
        let error = pool
            .prepare(bytes, 1, capabilities(), capabilities())
            .unwrap()
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::UnsupportedInterface);
        assert!(error.message.contains(expected), "{}", error.message);
        assert_eq!(pool.inner.memory.load(Ordering::Acquire), 0);
    }
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn preparation_rejects_invalid_replacements_before_activation() {
    let pool = WorkerPool::new().unwrap();
    let services = Arc::new(Services::default());
    let old = plugin_actor(&pool);
    old.invoke(request("status"), services.clone())
        .unwrap()
        .await
        .unwrap();
    let broken = pool
        .prepare(
            Arc::from(&b"invalid component"[..]),
            2,
            capabilities(),
            capabilities(),
        )
        .unwrap()
        .await;
    assert!(broken.is_err());
    assert!(old.is_active());
    old.invoke(request("status"), services.clone())
        .unwrap()
        .await
        .unwrap();
    let prepared = pool
        .prepare(fixture(), 1, capabilities(), capabilities())
        .unwrap()
        .await
        .unwrap();
    let new = pool.activate(prepared).unwrap();
    new.invoke(request("status"), services)
        .unwrap()
        .await
        .unwrap();
    old.shutdown().await.unwrap();
    new.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn unconsumed_large_results_hold_byte_quota_through_application() {
    let pool = WorkerPool::new().unwrap();
    let actor = plugin_actor(&pool);
    let services = Arc::new(Services::default());
    let first = actor
        .invoke(request("large-status"), services.clone())
        .unwrap()
        .await
        .unwrap();
    assert!(actor.inner._result_bytes.load(Ordering::Acquire) > 2 * 1024 * 1024);
    assert_eq!(
        actor
            .invoke(request("large-status"), services.clone())
            .unwrap()
            .await
            .unwrap_err()
            .code,
        ErrorCode::ResourceExhausted
    );
    assert!(actor.is_active());
    first.with_response(|_| {
        assert!(actor.inner._result_bytes.load(Ordering::Acquire) > 2 * 1024 * 1024);
    });
    assert_eq!(actor.inner._result_bytes.load(Ordering::Acquire), 0);
    drop(
        actor
            .invoke(request("large-status"), services)
            .unwrap()
            .await
            .unwrap(),
    );
    assert_eq!(pool.inner.result_bytes.load(Ordering::Acquire), 0);
    actor.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_during_job_creation_drops_the_future_without_child_work() {
    let pool = WorkerPool::new().unwrap();
    let actor = plugin_actor(&pool);
    let services = Arc::new(Services {
        creation_delay: Duration::from_secs(30),
        ..Services::default()
    });
    let completion = actor
        .invoke(request("job-start"), services.clone())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while services.creating.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    actor.cancel_target(Some(1), None);
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(250), completion)
            .await
            .unwrap()
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    actor.shutdown().await.unwrap();
    assert_eq!(services.creating.load(Ordering::Acquire), 0);
    assert_eq!(services.jobs.load(Ordering::Acquire), 0);
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn typed_ui_builders_bound_rows_and_check_each_builtin_capability() {
    let pool = WorkerPool::new().unwrap();
    let actor = plugin_actor(&pool);
    let services = Arc::new(Services::default());
    let response = actor
        .invoke(request("picker"), services.clone())
        .unwrap()
        .await
        .unwrap();
    assert!(
        matches!(&response.actions[0], Action::ShowUi { request: 17, kind: plugin_api::ui::UiKind::Picker { rows, .. }, .. } if rows.len() == 2 && rows[1].id == "1")
    );
    drop(response);
    assert_eq!(
        actor
            .invoke(request("picker-quota"), services.clone())
            .unwrap()
            .await
            .unwrap_err()
            .code,
        ErrorCode::ResourceExhausted
    );
    assert!(actor.is_active());
    let response = actor
        .invoke(request("keymap"), services.clone())
        .unwrap()
        .await
        .unwrap();
    assert!(
        matches!(&response.actions[0], Action::UpdateKeymap { request: 19, bindings } if bindings.len() == 1 && bindings[0].keys == ["space", "u"])
    );
    let grants = [Capability::Ui].into_iter().collect();
    let limited = pool.spawn(fixture(), 1, capabilities(), grants).unwrap();
    assert_eq!(
        limited
            .invoke(request("builtin"), services.clone())
            .unwrap()
            .await
            .unwrap_err()
            .code,
        ErrorCode::PermissionDenied
    );
    limited
        .invoke(request("status"), services)
        .unwrap()
        .await
        .unwrap();
    limited.shutdown().await.unwrap();
    actor.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn retained_results_are_bounded_across_plugins_when_consumer_stops() {
    let pool = WorkerPool::new().unwrap();
    let services = Arc::new(Services::default());
    let mut actors = Vec::new();
    let mut responses = Vec::new();
    let mut rejected = false;
    for _ in 0..16 {
        let actor = plugin_actor(&pool);
        match actor
            .invoke(request("large-status"), services.clone())
            .unwrap()
            .await
        {
            Ok(response) => responses.push(response),
            Err(failure) => {
                assert_eq!(failure.code, ErrorCode::ResourceExhausted);
                assert!(actor.is_active());
                rejected = true;
            }
        }
        actors.push(actor);
    }
    assert!(rejected);
    assert!(pool.inner.result_bytes.load(Ordering::Acquire) <= MAX_EDITOR_RESULTS);
    drop(responses);
    assert_eq!(pool.inner.result_bytes.load(Ordering::Acquire), 0);
    for actor in actors {
        actor.shutdown().await.unwrap();
    }
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn aggregate_guest_memory_is_bounded_across_plugins() {
    let pool = WorkerPool::new().unwrap();
    let services = Arc::new(Services::default());
    let mut actors = Vec::new();
    let mut rejected = false;
    for _ in 0..8 {
        let actor = plugin_actor(&pool);
        match actor
            .invoke(request("grow-32"), services.clone())
            .unwrap()
            .await
        {
            Ok(_) => actors.push(actor),
            Err(failure) => {
                assert_eq!(failure.code, ErrorCode::ResourceExhausted);
                assert!(!actor.is_active());
                rejected = true;
                actor.shutdown().await.unwrap();
                break;
            }
        }
    }
    assert!(rejected);
    assert!(!actors.is_empty());
    assert!(pool.inner.memory.load(Ordering::Acquire) <= component::MAX_TOTAL_MEMORY_BYTES);
    actors[0]
        .invoke(request("status"), services)
        .unwrap()
        .await
        .unwrap();
    for actor in actors {
        actor.shutdown().await.unwrap();
    }
    assert_eq!(pool.inner.memory.load(Ordering::Acquire), 0);
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn retained_jobs_notify_once_after_invocation_and_close_cancels_their_child() {
    let pool = WorkerPool::new().unwrap();
    let actor = plugin_actor(&pool);
    let services = Arc::new(Services {
        job_ready: true,
        ..Services::default()
    });
    drop(
        actor
            .invoke(request("job-start"), services.clone())
            .unwrap()
            .await
            .unwrap(),
    );
    assert_eq!(actor.job_target(1).unwrap().view, Some(1));
    actor
        .invoke(request("status"), services.clone())
        .unwrap()
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while services.notifications.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(*services.notifications.lock().unwrap(), [1]);
    assert_eq!(services.jobs.load(Ordering::Acquire), 1);
    actor.cancel_target(None, Some(1));
    tokio::time::timeout(Duration::from_secs(1), async {
        while services.jobs.load(Ordering::Acquire) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(actor.is_active());
    actor.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn closed_readiness_target_rejection_and_hidden_guest_target_clean_jobs() {
    let pool = WorkerPool::new().unwrap();
    let actor = plugin_actor(&pool);
    let rejected = Arc::new(Services {
        job_ready: true,
        reject_notification: true,
        ..Services::default()
    });
    actor
        .invoke(request("job-start"), rejected.clone())
        .unwrap()
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while rejected.jobs.load(Ordering::Acquire) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(actor.is_active());
    actor.shutdown().await.unwrap();
    let actor = plugin_actor(&pool);
    let services = Arc::new(Services::default());
    let mut hidden = request("job-start");
    hidden.editor.document = None;
    hidden.editor.view = None;
    actor
        .invoke_with_target(
            hidden,
            services.clone(),
            InvocationTarget {
                document: Some(21),
                view: Some(99),
                binding_revision: Some(4),
                call_sequence: None,
            },
        )
        .unwrap()
        .await
        .unwrap();
    actor.cancel_target(Some(21), None);
    tokio::time::timeout(Duration::from_secs(1), async {
        while services.jobs.load(Ordering::Acquire) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(actor.is_active());
    actor.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn retiring_navigation_exempts_only_its_current_call_and_cancels_jobs() {
    let pool = WorkerPool::new().unwrap();
    let actor = plugin_actor(&pool);
    let services = Arc::new(Services {
        read_delay: Duration::from_millis(50),
        ..Default::default()
    });
    let target = InvocationTarget {
        document: Some(1),
        view: Some(1),
        binding_revision: Some(9),
        call_sequence: Some(7),
    };
    actor
        .invoke_with_target(request("job-start"), services.clone(), target)
        .unwrap()
        .await
        .unwrap();
    let current = actor
        .invoke_with_target(request("uppercase"), services.clone(), target)
        .unwrap();
    let queued = actor
        .invoke_with_target(
            request("status"),
            services.clone(),
            InvocationTarget {
                call_sequence: Some(8),
                ..target
            },
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while services.starts.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    actor.cancel_target_except(Some(1), Some(1), Some(7));
    assert_eq!(queued.await.unwrap_err().code, ErrorCode::Cancelled);
    assert!(matches!(
        current.await.unwrap().actions.first(),
        Some(Action::Edit { .. })
    ));
    assert!(actor.is_active());
    tokio::time::timeout(Duration::from_secs(1), async {
        while services.jobs.load(Ordering::Acquire) != 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(actor.diagnostics().1.cancelled, 1);
    actor.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_initialization_disables_the_store_before_any_command() {
    let pool = WorkerPool::new().unwrap();
    let actor = plugin_actor(&pool);
    let mut initializing = request("loop");
    initializing.event = Event::Init;
    let completion = actor
        .invoke(initializing, Arc::new(Services::default()))
        .unwrap();
    drop(completion);
    if let Ok(command) = actor.invoke(request("status"), Arc::new(Services::default())) {
        assert!(command.await.is_err());
    }
    actor.shutdown().await.unwrap();
    assert!(!actor.is_active());
    pool.shutdown().await.unwrap();
}

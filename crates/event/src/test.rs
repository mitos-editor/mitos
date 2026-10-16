use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::{dispatch, events, register_dynamic_hook, register_event, register_hook};
#[tokio::test]
async fn smoke_test() {
    events! {
        Event1 { content: String }
        Event2 { content: usize }
    }
    register_event::<Event1>();
    register_event::<Event2>();

    // setup hooks
    let res1: Arc<Mutex<String>> = Arc::default();
    let acc = Arc::clone(&res1);
    register_hook!(move |event: &mut Event1| {
        acc.lock().push_str(&event.content);
        Ok(())
    });
    let res2: Arc<AtomicUsize> = Arc::default();
    let acc = Arc::clone(&res2);
    register_hook!(move |event: &mut Event2| {
        acc.fetch_add(event.content, Ordering::Relaxed);
        Ok(())
    });

    // triggers events
    let runtime = tokio::runtime::Handle::current();
    let thread = std::thread::spawn(move || {
        // Use the same event registry when integration tests isolate it by runtime.
        let _guard = runtime.enter();
        for i in 0..1000 {
            dispatch(Event2 { content: i }).log();
        }
    });
    std::thread::sleep(Duration::from_millis(1));
    dispatch(Event1 {
        content: "foo".to_owned(),
    })
    .log();
    dispatch(Event2 { content: 42 }).log();
    dispatch(Event1 {
        content: "bar".to_owned(),
    })
    .log();
    dispatch(Event1 {
        content: "hello world".to_owned(),
    })
    .log();
    thread.join().unwrap();

    // check output
    assert_eq!(&**res1.lock(), "foobarhello world");
    assert_eq!(
        res2.load(Ordering::Relaxed),
        42 + (0..1000usize).sum::<usize>()
    );
}

#[tokio::test]
#[allow(dead_code)]
async fn dynamic() {
    events! {
        Event3 {}
        Event4 { count: usize }
    };
    register_event::<Event3>();
    register_event::<Event4>();

    let count = Arc::new(AtomicUsize::new(0));
    let count1 = count.clone();
    let count2 = count.clone();
    let _event3_hook = register_dynamic_hook(
        move || {
            count1.fetch_add(2, Ordering::Relaxed);
            Ok(())
        },
        "Event3",
    )
    .unwrap();
    let _event4_hook = register_dynamic_hook(
        move || {
            count2.fetch_add(3, Ordering::Relaxed);
            Ok(())
        },
        "Event4",
    )
    .unwrap();
    dispatch(Event3 {}).log();
    dispatch(Event4 { count: 0 }).log();
    dispatch(Event3 {}).log();
    assert_eq!(count.load(Ordering::Relaxed), 7)
}

#[tokio::test]
async fn dropped_registration_stops_delivery_and_releases_captures() {
    events! { OwnedEvent {} }
    register_event::<OwnedEvent>();
    let count = Arc::new(AtomicUsize::new(0));
    let captured = count.clone();
    let weak = Arc::downgrade(&count);
    let registration = register_dynamic_hook(
        move || {
            captured.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
        "OwnedEvent",
    )
    .unwrap();
    dispatch(OwnedEvent {}).log();
    assert_eq!(count.load(Ordering::Relaxed), 1);
    // A registration keeps its originating registry even without a Tokio runtime.
    std::thread::spawn(move || drop(registration))
        .join()
        .unwrap();
    dispatch(OwnedEvent {}).log();
    assert_eq!(count.load(Ordering::Relaxed), 1);
    drop(count);
    assert!(weak.upgrade().is_none());
}

#[tokio::test]
async fn hooks_can_remove_other_hooks_and_subscribe_during_dispatch() {
    events! { ReentrantEvent {} }
    register_event::<ReentrantEvent>();
    let order = Arc::new(Mutex::new(Vec::new()));
    let later = Arc::new(Mutex::new(None));
    let added = Arc::new(Mutex::new(None));
    let observed = order.clone();
    let remove = later.clone();
    let registrations = added.clone();
    let _first = crate::register_owned_hook!(move |_event: &mut ReentrantEvent| {
        observed.lock().push(1);
        // Removing a pending hook skips it in this dispatch's snapshot.
        remove.lock().take();
        if registrations.lock().is_none() {
            let observed = observed.clone();
            *registrations.lock() = Some(register_dynamic_hook(
                move || {
                    observed.lock().push(3);
                    Ok(())
                },
                "ReentrantEvent",
            )?);
        }
        Ok(())
    });
    let observed = order.clone();
    *later.lock() = Some(
        register_dynamic_hook(
            move || {
                observed.lock().push(2);
                Ok(())
            },
            "ReentrantEvent",
        )
        .unwrap(),
    );
    dispatch(ReentrantEvent {}).log();
    assert_eq!(*order.lock(), [1]);
    dispatch(ReentrantEvent {}).log();
    assert_eq!(*order.lock(), [1, 1, 3]);
    // Break the test's ownership cycle while exercising removal outside dispatch.
    added.lock().take();
}

#[tokio::test]
async fn hooks_can_unsubscribe_themselves_and_dispatch_recursively() {
    events! { RecursiveEvent { depth: usize } }
    register_event::<RecursiveEvent>();
    let own = Arc::new(Mutex::new(None));
    let registration = own.clone();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    *own.lock() = Some(crate::register_owned_hook!(
        move |event: &mut RecursiveEvent| {
            observed.fetch_add(1, Ordering::Relaxed);
            registration.lock().take();
            if event.depth == 0 {
                dispatch(RecursiveEvent { depth: 1 }).log();
            }
            Ok(())
        }
    ));
    dispatch(RecursiveEvent { depth: 0 }).log();
    dispatch(RecursiveEvent { depth: 0 }).log();
    assert_eq!(count.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn unsubscribing_releases_nested_registrations_outside_the_registry_lock() {
    events! { NestedLifetimeEvent {} }
    register_event::<NestedLifetimeEvent>();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let inner = register_dynamic_hook(
        move || {
            observed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
        "NestedLifetimeEvent",
    )
    .unwrap();
    let outer = register_dynamic_hook(
        move || {
            let _keep_alive = &inner;
            Ok(())
        },
        "NestedLifetimeEvent",
    )
    .unwrap();
    dispatch(NestedLifetimeEvent {}).log();
    assert_eq!(count.load(Ordering::Relaxed), 1);
    drop(outer);
    dispatch(NestedLifetimeEvent {}).log();
    assert_eq!(count.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn dropping_a_registration_allows_an_active_callback_to_finish() {
    events! { ConcurrentLifetimeEvent {} }
    register_event::<ConcurrentLifetimeEvent>();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let callback_barrier = barrier.clone();
    let registration = register_dynamic_hook(
        move || {
            observed.fetch_add(1, Ordering::Relaxed);
            callback_barrier.wait();
            callback_barrier.wait();
            Ok(())
        },
        "ConcurrentLifetimeEvent",
    )
    .unwrap();
    let runtime = tokio::runtime::Handle::current();
    let thread = std::thread::spawn(move || {
        let _runtime = runtime.enter();
        dispatch(ConcurrentLifetimeEvent {}).log();
    });
    barrier.wait();
    drop(registration);
    barrier.wait();
    thread.join().unwrap();
    dispatch(ConcurrentLifetimeEvent {}).log();
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert_eq!(Arc::strong_count(&count), 1);
}

#[tokio::test]
async fn failed_registration_releases_captures_without_locking_out_unsubscription() {
    events! { FailedLifetimeEvent {} }
    register_event::<FailedLifetimeEvent>();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let inner = register_dynamic_hook(
        move || {
            observed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
        "FailedLifetimeEvent",
    )
    .unwrap();
    let result = register_dynamic_hook(
        move || {
            let _keep_alive = &inner;
            Ok(())
        },
        "MissingLifetimeEvent",
    );
    assert!(result.is_err());
    dispatch(FailedLifetimeEvent {}).log();
    assert_eq!(count.load(Ordering::Relaxed), 0);
    assert_eq!(Arc::strong_count(&count), 1);
}

#[tokio::test]
async fn failed_typed_registration_releases_captures_without_deadlocking() {
    const CHILD: &str = "MITOS_TEST_TYPED_REGISTRATION_PANIC_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // A regression would deadlock the process-global registry. Isolate it so
        // this test reports a bounded failure instead of hanging the entire suite.
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "test::failed_typed_registration_releases_captures_without_deadlocking",
            ])
            .env(CHILD, "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "typed registration panic cleanup failed");
                return;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("typed registration deadlocked while releasing captured subscriptions");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    events! {
        TypedFailureLifetimeEvent {}
        MissingTypedFailureLifetimeEvent {}
    }
    register_event::<TypedFailureLifetimeEvent>();
    for owned in [false, true] {
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        let inner = register_dynamic_hook(
            move || {
                observed.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
            "TypedFailureLifetimeEvent",
        )
        .unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if owned {
                let _registration = crate::register_owned_hook!(
                    move |_event: &mut MissingTypedFailureLifetimeEvent| {
                        let _keep_alive = &inner;
                        Ok(())
                    }
                );
            } else {
                register_hook!(move |_event: &mut MissingTypedFailureLifetimeEvent| {
                    let _keep_alive = &inner;
                    Ok(())
                });
            }
        }));
        assert!(result.is_err());
        dispatch(TypedFailureLifetimeEvent {}).log();
        assert_eq!(count.load(Ordering::Relaxed), 0);
        assert_eq!(Arc::strong_count(&count), 1);
    }
}

#[tokio::test]
async fn hook_failures_report_to_the_selected_owner_after_all_observers_run() {
    events! { ReportedFailureEvent {} }
    register_event::<ReportedFailureEvent>();
    let _failing =
        register_dynamic_hook(|| anyhow::bail!("observer failed"), "ReportedFailureEvent").unwrap();
    let called = Arc::new(AtomicUsize::new(0));
    let observed = called.clone();
    let _following = register_dynamic_hook(
        move || {
            observed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
        "ReportedFailureEvent",
    )
    .unwrap();
    let (_first, mut first_messages) = crate::status::StatusSender::channel();
    let (second, mut second_messages) = crate::status::StatusSender::channel();
    dispatch(ReportedFailureEvent {}).report_to(&second);
    assert_eq!(called.load(Ordering::Relaxed), 1);
    assert_eq!(
        second_messages.try_recv().unwrap().message,
        "observer failed"
    );
    assert!(first_messages.try_recv().is_err());
}

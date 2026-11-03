use crate::{events, register_hook, register_owned_hook, EventHub};
use parking_lot::Mutex;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[test]
fn typed_hooks_accept_independent_borrowed_lifetimes() {
    events! { Borrowed<'a, 'b> { output: &'a mut String, text: &'b str } }
    let hub = EventHub::default();
    let registration = register_owned_hook!(&hub => move |event: &mut Borrowed<'_, '_>| {
        event.output.push_str(event.text);
        Ok(())
    });
    let text = String::from("borrowed");
    let mut output = String::new();
    hub.dispatch(Borrowed {
        output: &mut output,
        text: &text,
    })
    .log();
    assert_eq!(output, "borrowed");
    drop(registration);
    hub.dispatch(Borrowed {
        output: &mut output,
        text: &text,
    })
    .log();
    assert_eq!(output, "borrowed");
}

#[tokio::test]
async fn hubs_are_isolated_within_one_runtime_and_clones_work_without_a_runtime() {
    events! { Increment { amount: usize } }
    let first = EventHub::default();
    let second = EventHub::default();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    register_hook!(&first => move |event: &mut Increment| {
        observed.fetch_add(event.amount, Ordering::Relaxed);
        Ok(())
    });
    second.dispatch(Increment { amount: 100 }).log();
    let worker = first.clone();
    std::thread::spawn(move || {
        for amount in 0..1000 {
            worker.dispatch(Increment { amount }).log();
        }
    })
    .join()
    .unwrap();
    first.dispatch(Increment { amount: 42 }).log();
    assert_eq!(count.load(Ordering::Relaxed), 42 + (0..1000).sum::<usize>());
}

#[test]
fn dropped_registration_stops_delivery_and_releases_captures() {
    events! { Owned {} }
    let hub = EventHub::default();
    let count = Arc::new(AtomicUsize::new(0));
    let captured = count.clone();
    let registration = register_owned_hook!(&hub => move |_event: &mut Owned| {
        captured.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    hub.dispatch(Owned {}).log();
    std::thread::spawn(move || drop(registration))
        .join()
        .unwrap();
    hub.dispatch(Owned {}).log();
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert_eq!(Arc::strong_count(&count), 1);
}

#[test]
fn dropping_the_hub_releases_permanent_hooks_and_subscriptions_can_outlive_it() {
    events! { Lifetime {} }
    let hub = EventHub::default();
    let captured = Arc::new(());
    let owned = captured.clone();
    register_hook!(&hub => move |_event: &mut Lifetime| {
        let _ = &owned;
        Ok(())
    });
    let registration = register_owned_hook!(&hub => move |_event: &mut Lifetime| Ok(()));
    assert_eq!(Arc::strong_count(&captured), 2);
    drop(hub);
    assert_eq!(Arc::strong_count(&captured), 1);
    drop(registration);
}

#[test]
fn hooks_can_remove_other_hooks_and_subscribe_during_dispatch() {
    events! { Reentrant {} }
    let hub = EventHub::default();
    let order = Arc::new(Mutex::new(Vec::new()));
    let later = Arc::new(Mutex::new(None));
    let added = Arc::new(Mutex::new(None));
    let observed = order.clone();
    let remove = later.clone();
    let registrations = added.clone();
    // Owned callbacks may capture their hub. Dropping the returned subscription breaks the cycle.
    let destination = hub.clone();
    let first = register_owned_hook!(&hub => move |_event: &mut Reentrant| {
        observed.lock().push(1);
        remove.lock().take();
        if registrations.lock().is_none() {
            let observed = observed.clone();
            *registrations.lock() = Some(register_owned_hook!(
                &destination =>
                move |_event: &mut Reentrant| {
                    observed.lock().push(3);
                    Ok(())
                }
            ));
        }
        Ok(())
    });
    let observed = order.clone();
    *later.lock() = Some(register_owned_hook!(&hub => move |_event: &mut Reentrant| {
        observed.lock().push(2);
        Ok(())
    }));
    hub.dispatch(Reentrant {}).log();
    assert_eq!(*order.lock(), [1]);
    hub.dispatch(Reentrant {}).log();
    assert_eq!(*order.lock(), [1, 1, 3]);
    drop(first);
    added.lock().take();
}

#[test]
fn hooks_can_unsubscribe_themselves_and_dispatch_recursively() {
    events! { Recursive { depth: usize } }
    let hub = EventHub::default();
    let own = Arc::new(Mutex::new(None));
    let registration = own.clone();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let destination = hub.clone();
    *own.lock() = Some(register_owned_hook!(&hub => move |event: &mut Recursive| {
        observed.fetch_add(1, Ordering::Relaxed);
        registration.lock().take();
        if event.depth == 0 {
            destination.dispatch(Recursive { depth: 1 }).log();
        }
        Ok(())
    }));
    hub.dispatch(Recursive { depth: 0 }).log();
    hub.dispatch(Recursive { depth: 0 }).log();
    assert_eq!(count.load(Ordering::Relaxed), 1);
}

#[test]
fn unsubscribing_releases_nested_registrations_outside_the_registry_lock() {
    events! { Nested {} }
    let hub = EventHub::default();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let inner = register_owned_hook!(&hub => move |_event: &mut Nested| {
        observed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    let outer = register_owned_hook!(&hub => move |_event: &mut Nested| {
        let _ = &inner;
        Ok(())
    });
    hub.dispatch(Nested {}).log();
    drop(outer);
    hub.dispatch(Nested {}).log();
    assert_eq!(count.load(Ordering::Relaxed), 1);
}

#[test]
fn dropping_a_registration_allows_an_active_callback_to_finish() {
    events! { Concurrent {} }
    let hub = EventHub::default();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let callback_barrier = barrier.clone();
    let registration = register_owned_hook!(&hub => move |_event: &mut Concurrent| {
        observed.fetch_add(1, Ordering::Relaxed);
        callback_barrier.wait();
        callback_barrier.wait();
        Ok(())
    });
    let worker = hub.clone();
    let thread = std::thread::spawn(move || worker.dispatch(Concurrent {}).log());
    barrier.wait();
    drop(registration);
    barrier.wait();
    thread.join().unwrap();
    hub.dispatch(Concurrent {}).log();
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert_eq!(Arc::strong_count(&count), 1);
}

#[test]
fn hook_failures_report_to_the_selected_owner_after_all_observers_run() {
    events! { Failure {} }
    let hub = EventHub::default();
    register_hook!(&hub => move |_event: &mut Failure| anyhow::bail!(
        "observer failed"
    ));
    let called = Arc::new(AtomicUsize::new(0));
    let observed = called.clone();
    register_hook!(&hub => move |_event: &mut Failure| {
        observed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    let (_first, mut first_messages) = crate::status::StatusSender::channel();
    let (second, mut second_messages) = crate::status::StatusSender::channel();
    hub.dispatch(Failure {}).report_to(&second);
    assert_eq!(called.load(Ordering::Relaxed), 1);
    assert_eq!(
        second_messages.try_recv().unwrap().message,
        "observer failed"
    );
    assert!(first_messages.try_recv().is_err());
}

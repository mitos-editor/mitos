//! Frontend signals belong to the editor that created their destination.

use std::{sync::Arc, time::Duration};

use super::Fixture;
use ::diagnostics::Severity;
use event::status::{Severity as StatusSeverity, StatusMessage};
use view::{events::DocumentDidChange, Editor};

async fn receive_status(editor: &mut Editor) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(1), async {
        while editor.get_status().is_none() {
            editor.wait_event().await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn editors_in_one_runtime_receive_only_their_own_status_and_redraws() -> anyhow::Result<()> {
    let mut first = Fixture::new("first\n")?;
    let mut second = Fixture::new("second\n")?;
    first.editor.redraw().start_frame();
    second.editor.redraw().start_frame();
    view::current_ref!(first.editor).1.redraw().request();
    tokio::time::timeout(Duration::from_secs(1), first.editor.redraw().requested()).await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(5), second.editor.redraw().requested())
            .await
            .is_err()
    );
    first
        .editor
        .status_sender()
        .report(StatusMessage {
            severity: StatusSeverity::Warning,
            message: "first warning".into(),
        })
        .await;
    receive_status(&mut first.editor).await?;
    assert_eq!(
        first
            .editor
            .get_status()
            .map(|(message, severity)| (message.as_ref(), *severity)),
        Some(("first warning", Severity::Warning))
    );
    assert!(second.editor.get_status().is_none());
    second.editor.status_sender().report("second message").await;
    receive_status(&mut second.editor).await?;
    assert_eq!(
        second
            .editor
            .get_status()
            .map(|(message, _)| message.as_ref()),
        Some("second message")
    );

    let closed_status = first.editor.status_sender().clone();
    let closed_redraw = first.editor.redraw().callback();
    second.editor.redraw().start_frame();
    drop(first);
    closed_status.report("closed").await;
    std::thread::spawn(closed_redraw).join().unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(5), second.editor.redraw().requested())
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn document_hook_failures_are_reported_to_the_document_owner() -> anyhow::Result<()> {
    let mut first = Fixture::new("first\n")?;
    let mut second = Fixture::new("second\n")?;
    let _registration = event::register_owned_hook!(
        second.editor.events() =>
        move |_event: &mut DocumentDidChange<'_>| { anyhow::bail!("document observer failed") }
    );
    second.replace("changed\n");
    receive_status(&mut second.editor).await?;
    assert_eq!(
        second
            .editor
            .get_status()
            .map(|(message, severity)| (message.as_ref(), *severity)),
        Some(("document observer failed", Severity::Error))
    );
    assert!(first.editor.get_status().is_none());
    first
        .editor
        .status_sender()
        .report("first remains alive")
        .await;
    receive_status(&mut first.editor).await?;
    assert_eq!(
        first
            .editor
            .get_status()
            .map(|(message, _)| message.as_ref()),
        Some("first remains alive")
    );
    Ok(())
}

#[tokio::test]
async fn editors_in_one_runtime_route_document_hooks_and_release_their_hubs() -> anyhow::Result<()>
{
    let mut first = Fixture::new("first\n")?;
    let mut second = Fixture::new("second\n")?;
    let first_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let second_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = first_calls.clone();
    event::register_hook!(
        first.editor.events() =>
        move |_event: &mut DocumentDidChange<'_>| {
            observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
    );
    let observed = second_calls.clone();
    event::register_hook!(
        second.editor.events() =>
        move |_event: &mut DocumentDidChange<'_>| {
            observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
    );
    first.replace("first changed\n");
    assert_eq!(first_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(second_calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    second.replace("second changed\n");
    assert_eq!(first_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(second_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    drop(first);
    assert_eq!(Arc::strong_count(&first_calls), 1);
    second.replace("second remains alive\n");
    assert_eq!(second_calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    Ok(())
}

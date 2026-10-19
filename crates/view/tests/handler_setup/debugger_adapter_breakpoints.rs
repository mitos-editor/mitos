use super::*;
use view::{
    current_ref,
    debugger::{BreakpointProperty, DebuggerSource},
};

async fn adapter_source(
    f: &mut Fixture,
    previous: Option<view::DocumentId>,
) -> anyhow::Result<DebuggerSource> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let doc = current_ref!(f.editor).1;
            if Some(doc.id()) != previous
                && let Some(source) = f.editor.debugger_source(doc.id())
                && source.local_path().is_none()
            {
                return source;
            }
            f.callbacks.recv().await.unwrap()(&mut f.editor);
        }
    })
    .await
    .map_err(Into::into)
}

#[tokio::test(flavor = "multi_thread")]
async fn adapter_breakpoints_target_the_owning_session_and_never_the_coincidental_disk_path(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("disk text")?;
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let (first, mut wire) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            stack_reference: Some(7),
            stack_source: Some(path.clone()),
            source_content: "generated\n".into(),
            ..Default::default()
        },
    )
    .await?;
    let source = adapter_source(&mut f, None).await?;
    let (second, mut other_wire) = paused_adapter(&mut f).await?;
    f.editor.select_debugger_session(second)?;
    f.editor.toggle_source_breakpoints(&source, &[0, 0])?;
    let request = command(&mut wire, "setBreakpoints").await?;
    assert_eq!(request["arguments"]["source"]["sourceReference"], 7);
    assert_eq!(
        request["arguments"]["breakpoints"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(request["arguments"]["breakpoints"][0]["line"], 1);
    debugger_callback(&mut f).await?;
    let locations = f.editor.debugger_breakpoint_locations(&source);
    assert_eq!(locations.len(), 1);
    assert!(locations[0].verified);
    assert!(f.editor.debugger().breakpoints(&path).is_empty());
    let identity = locations[0].definition.as_ref().unwrap().identity();
    f.editor
        .handle_debugger_message(
            first,
            debugger_event(
                "capabilities",
                json!({"capabilities":{"supportsConditionalBreakpoints":true}}),
            ),
        )
        .await;
    f.editor.set_source_breakpoint_property(
        &source,
        identity,
        BreakpointProperty::Condition,
        Some("ready".into()),
    )?;
    assert_eq!(
        command(&mut wire, "setBreakpoints").await?["arguments"]["breakpoints"][0]["condition"],
        "ready"
    );
    debugger_callback(&mut f).await?;
    f.editor.toggle_source_breakpoints(&source, &[0])?;
    assert_eq!(
        command(&mut wire, "setBreakpoints").await?["arguments"]["breakpoints"],
        json!([])
    );
    debugger_callback(&mut f).await?;
    assert!(f.editor.debugger_breakpoint_locations(&source).is_empty());
    // A wire barrier proves no breakpoint request was broadcast to the active session.
    f.editor
        .debugger_threads()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    while let Ok(message) = other_wire.try_recv() {
        assert_ne!(message["command"], "setBreakpoints");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn replaced_sources_revoke_queued_bindings_and_old_breakpoint_handles() -> anyhow::Result<()>
{
    let mut f = Fixture::new("")?;
    let (id, mut wire) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            stack_reference: Some(7),
            source_content: "generated".into(),
            ..Default::default()
        },
    )
    .await?;
    let source = adapter_source(&mut f, None).await?;
    let document = current_ref!(f.editor).1.id();
    f.editor.toggle_source_breakpoints(&source, &[0])?;
    command(&mut wire, "setBreakpoints").await?;
    let old = tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
        .await?
        .unwrap();
    f.editor
        .handle_debugger_message(
            id,
            debugger_event(
                "loadedSource",
                json!({"reason":"changed","source":{"sourceReference":7}}),
            ),
        )
        .await;
    let frames = f.editor.debugger_stack_frames()?;
    f.editor.select_debugger_frame(&frames.context, 0)?;
    let refreshed = adapter_source(&mut f, Some(document)).await?;
    old(&mut f.editor);
    assert!(f.editor.toggle_source_breakpoints(&source, &[0]).is_err());
    assert!(f
        .editor
        .debugger_breakpoint_locations(&refreshed)
        .is_empty());
    f.editor.toggle_source_breakpoints(&refreshed, &[0])?;
    command(&mut wire, "setBreakpoints").await?;
    debugger_callback(&mut f).await?;
    let identity = f.editor.debugger_breakpoint_locations(&refreshed)[0]
        .definition
        .as_ref()
        .unwrap()
        .identity();
    f.editor
        .handle_debugger_event(id, dap::ClientEvent::Closed { error: None })
        .await;
    assert!(!refreshed.is_current(&f.editor));
    assert!(f.editor.debugger().breakpoint_binding(identity).is_none());
    assert!(f
        .editor
        .toggle_source_breakpoints(&refreshed, &[0])
        .is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn adapter_breakpoint_edits_wait_for_in_flight_requests_before_sending_the_latest_definition(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (id, mut wire) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            stack_reference: Some(7),
            source_content: "generated".into(),
            held_commands: vec!["setBreakpoints"],
            ..Default::default()
        },
    )
    .await?;
    let source = adapter_source(&mut f, None).await?;
    f.editor.toggle_source_breakpoints(&source, &[0])?;
    command(&mut wire, "setBreakpoints").await?;
    f.editor.toggle_source_breakpoints(&source, &[0])?;
    release_adapter_request(&wire).await?;
    debugger_callback(&mut f).await?;
    assert_eq!(
        command(&mut wire, "setBreakpoints").await?["arguments"]["breakpoints"],
        json!([])
    );
    debugger_callback(&mut f).await?;
    assert!(f.editor.debugger_breakpoint_locations(&source).is_empty());
    assert!(f.editor.debugger().get_session(id).is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn discovered_adapter_breakpoints_are_isolated_from_sources_with_the_same_reference(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (first, _) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            stack_reference: Some(7),
            source_content: "first".into(),
            ..Default::default()
        },
    )
    .await?;
    let source = adapter_source(&mut f, None).await?;
    let previous = current_ref!(f.editor).1.id();
    let (second, _) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            stack_reference: Some(7),
            source_content: "second".into(),
            ..Default::default()
        },
    )
    .await?;
    let other = adapter_source(&mut f, Some(previous)).await?;
    f.editor.handle_debugger_message(first, debugger_event("breakpoint", json!({"reason":"new","breakpoint":{"id":77,"verified":true,"line":1,"source":{"sourceReference":7}}}))).await;
    let discovered = f.editor.debugger_breakpoint_locations(&source);
    assert_eq!(discovered.len(), 1);
    assert!(discovered[0].definition.is_none());
    assert!(f.editor.debugger_breakpoint_locations(&other).is_empty());
    f.editor.toggle_source_breakpoints(&other, &[0])?;
    debugger_callback(&mut f).await?;
    assert_eq!(f.editor.debugger_breakpoint_locations(&other).len(), 1);
    f.editor
        .handle_debugger_event(first, dap::ClientEvent::Closed { error: None })
        .await;
    assert!(!source.is_current(&f.editor));
    assert!(other.is_current(&f.editor));
    assert_eq!(f.editor.debugger_breakpoint_locations(&other).len(), 1);
    assert_eq!(
        f.editor.debugger().get_active_session().unwrap().id(),
        second
    );
    Ok(())
}

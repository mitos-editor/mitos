use super::*;
use view::debugger::DebuggerCommand;

#[tokio::test(flavor = "multi_thread")]
async fn exception_settings_preserve_inspection_and_serialize_updates_across_frame_changes(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (id, mut wire) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            held_commands: vec!["setExceptionBreakpoints"],
            ..Default::default()
        },
    )
    .await?;
    f.editor.handle_debugger_message(id, debugger_event("capabilities", json!({"capabilities":{"exceptionBreakpointFilters":[{"filter":"all","label":"All"}]}}))).await;
    let page = f
        .editor
        .debugger_variables()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    command(&mut wire, "scopes").await?;
    f.editor
        .debugger_command(DebuggerCommand::Exceptions(true))?;
    assert!(page.is_current(&f.editor));
    let first = command(&mut wire, "setExceptionBreakpoints").await?;
    assert_eq!(first["arguments"]["filters"], json!(["all"]));
    let frames = f.editor.debugger_stack_frames()?;
    f.editor.select_debugger_frame(&frames.context, 1)?;
    f.editor
        .debugger_command(DebuggerCommand::Exceptions(false))?;
    release_adapter_request(&wire).await?;
    debugger_callback(&mut f).await?;
    let latest = command(&mut wire, "setExceptionBreakpoints").await?;
    assert_eq!(latest["arguments"]["filters"], json!([]));
    debugger_callback(&mut f).await?;
    // Reinitialization restores the desired setting before configurationDone.
    f.editor
        .handle_debugger_message(id, debugger_event("initialized", json!({})))
        .await;
    assert_eq!(
        command(&mut wire, "setExceptionBreakpoints").await?["arguments"]["filters"],
        json!([])
    );
    debugger_callback(&mut f).await?;
    command(&mut wire, "configurationDone").await?;
    debugger_callback(&mut f).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn execution_control_acknowledgements_survive_frame_navigation() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (_, mut wire) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            held_commands: vec!["continue"],
            ..Default::default()
        },
    )
    .await?;
    f.editor.debugger_command(DebuggerCommand::Continue)?;
    command(&mut wire, "continue").await?;
    let frames = f.editor.debugger_stack_frames()?;
    f.editor.select_debugger_frame(&frames.context, 1)?;
    release_adapter_request(&wire).await?;
    debugger_callback(&mut f).await?;
    assert!(f
        .editor
        .debugger()
        .get_active_session()
        .unwrap()
        .current_stack_frame()
        .is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_shutdown_retries_pending_session_configuration() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (_, mut wire) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            held_commands: vec!["setExceptionBreakpoints", "terminate"],
            failed_commands: vec!["terminate"],
            ..Default::default()
        },
    )
    .await?;
    f.editor
        .debugger_command(DebuggerCommand::Exceptions(true))?;
    command(&mut wire, "setExceptionBreakpoints").await?;
    f.editor
        .debugger_command(DebuggerCommand::Exceptions(false))?;
    f.editor.debugger_command(DebuggerCommand::Terminate)?;
    command(&mut wire, "terminate").await?;
    release_adapter_request(&wire).await?;
    debugger_callback(&mut f).await?;
    assert_eq!(
        command(&mut wire, "setExceptionBreakpoints").await?["arguments"]["filters"],
        json!([])
    );
    debugger_callback(&mut f).await?;
    assert!(f.editor.debugger_threads().is_ok());
    Ok(())
}

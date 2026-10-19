use super::*;
use view::debugger::DebuggerCommand;

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_revokes_queued_breakpoint_results_and_rejection_restores_configuration(
) -> anyhow::Result<()> {
    for rejected in [false, true] {
        let mut f = Fixture::new("marked\n")?;
        let path = view::current_ref!(f.editor).1.path().unwrap().to_owned();
        f.editor.toggle_breakpoints(path.clone(), &[0])?;
        let (id, mut wire) = adapter_with_options(
            &mut f,
            AdapterOptions {
                held_commands: vec!["terminate"],
                failed_commands: if rejected { vec!["terminate"] } else { vec![] },
                ..Default::default()
            },
        )
        .await?;
        f.editor
            .handle_debugger_message(id, debugger_event("initialized", json!({})))
            .await;
        command(&mut wire, "setBreakpoints").await?;
        let queued = tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
            .await?
            .unwrap();
        f.editor.debugger_command(DebuggerCommand::Terminate)?;
        command(&mut wire, "terminate").await?;
        f.editor.set_status("shutting down");
        queued(&mut f.editor);
        // Remove the final marker while shutdown is pending. A rejected shutdown
        // must send the deferred clear before completing configuration.
        f.editor.toggle_breakpoints(path, &[0])?;
        assert_eq!(f.editor.get_status().unwrap().0, "shutting down");
        release_adapter_request(&wire).await?;
        debugger_callback(&mut f).await?;
        if rejected {
            let clear = command(&mut wire, "setBreakpoints").await?;
            assert_eq!(clear["arguments"]["breakpoints"], json!([]));
            debugger_callback(&mut f).await?;
            command(&mut wire, "configurationDone").await?;
            debugger_callback(&mut f).await?;
            assert!(f.editor.debugger_threads().is_ok());
        } else {
            assert!(f.editor.debugger().get_session(id).is_none());
            assert!(wire.try_recv().is_err());
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_configuration_acknowledgements_are_silent_during_shutdown() -> anyhow::Result<()> {
    for rejected in [false, true] {
        let mut f = Fixture::new("")?;
        let (id, mut wire) = adapter_with_options(
            &mut f,
            AdapterOptions {
                held_commands: vec!["terminate"],
                failed_commands: if rejected { vec!["terminate"] } else { vec![] },
                ..Default::default()
            },
        )
        .await?;
        f.editor
            .handle_debugger_message(id, debugger_event("initialized", json!({})))
            .await;
        command(&mut wire, "configurationDone").await?;
        let queued = tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
            .await?
            .unwrap();
        f.editor.debugger_command(DebuggerCommand::Terminate)?;
        command(&mut wire, "terminate").await?;
        f.editor.set_status("shutting down");
        queued(&mut f.editor);
        assert_eq!(f.editor.get_status().unwrap().0, "shutting down");
        release_adapter_request(&wire).await?;
        debugger_callback(&mut f).await?;
        assert!(wire.try_recv().is_err());
        assert_eq!(f.editor.debugger().get_session(id).is_some(), rejected);
    }
    Ok(())
}

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn scoped_invalidation_revokes_queued_values_and_refreshes_open_ancestry(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (id, _) = paused_adapter(&mut f).await?;
    let root = f
        .editor
        .debugger_variables()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    let locals = f
        .editor
        .debugger_variable_children(&root, 0)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    let nested = f
        .editor
        .debugger_variable_children(&locals, 1)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    let queued = f
        .editor
        .debugger_variable_children(&locals, 1)?
        .run()
        .await
        .unwrap();
    f.editor
        .handle_debugger_message(
            id,
            debugger_event("invalidated", json!({"areas":["variables"],"threadId":99})),
        )
        .await;
    assert!(nested.is_current(&f.editor));
    f.editor
        .handle_debugger_message(
            id,
            debugger_event(
                "invalidated",
                json!({"areas":["variables"],"stackFrameId":999}),
            ),
        )
        .await;
    assert!(nested.is_current(&f.editor));
    f.editor
        .handle_debugger_message(
            id,
            debugger_event(
                "invalidated",
                json!({"areas":["variables"],"threadId":99,"stackFrameId":101}),
            ),
        )
        .await;
    assert!(!nested.is_current(&f.editor));
    assert!(nested.can_refresh(&f.editor));
    assert!(queued.into_result(&f.editor).is_none());
    let pages = f
        .editor
        .debugger_refresh_variables(&nested)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(pages.len(), 3);
    assert!(pages.iter().all(|page| page.is_current(&f.editor)));
    assert_eq!(pages[2].entries()[0].value.as_deref(), Some("nested value"));
    f.editor
        .debugger_evaluate("expression".into())?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert!(!pages[2].is_current(&f.editor));
    assert!(pages[2].can_refresh(&f.editor));
    f.editor
        .handle_debugger_message(id, debugger_event("continued", json!({"threadId":1})))
        .await;
    assert!(!pages[2].can_refresh(&f.editor));
    assert!(f.editor.debugger_refresh_variables(&pages[2]).is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stack_invalidation_retains_selected_frame_and_stop_reason() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (id, mut wire) = paused_adapter(&mut f).await?;
    let frames = f.editor.debugger_stack_frames()?;
    f.editor.select_debugger_frame(&frames.context, 1)?;
    let selected = f
        .editor
        .debugger()
        .get_session(id)
        .unwrap()
        .current_stack_frame()
        .unwrap()
        .id;
    f.editor
        .handle_debugger_message(
            id,
            debugger_event("invalidated", json!({"areas":["stacks"]})),
        )
        .await;
    assert!(!frames.context.is_current(&f.editor));
    command(&mut wire, "stackTrace").await?;
    tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
        .await?
        .unwrap()(&mut f.editor);
    let session = f.editor.debugger().get_session(id).unwrap();
    assert_eq!(session.current_stack_frame().unwrap().id, selected);
    assert!(session
        .thread_states()
        .values()
        .all(|state| state.is_stopped()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn background_stack_refresh_revokes_saved_frame_indices() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (first, mut wire) = paused_adapter(&mut f).await?;
    let frames = f.editor.debugger_stack_frames()?;
    let (second, _) = paused_adapter(&mut f).await?;
    f.editor.select_debugger_session(second)?;
    f.editor
        .handle_debugger_message(
            first,
            debugger_event("invalidated", json!({"areas":["stacks"]})),
        )
        .await;
    command(&mut wire, "stackTrace").await?;
    tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
        .await?
        .unwrap()(&mut f.editor);
    assert!(!frames.context.is_current(&f.editor));
    assert_eq!(
        f.editor.debugger().get_active_session().unwrap().id(),
        second
    );
    assert!(f
        .editor
        .debugger()
        .get_session(first)
        .unwrap()
        .current_stack_frame()
        .is_some());
    Ok(())
}

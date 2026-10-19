use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn traces_load_bounded_ranges_and_keep_selection_for_known_and_unknown_totals(
) -> anyhow::Result<()> {
    for unknown_stack_total in [false, true] {
        let mut f = Fixture::new("")?;
        let (id, mut wire) = adapter_with_options(
            &mut f,
            AdapterOptions {
                stack_count: Some(70),
                unknown_stack_total,
                ..Default::default()
            },
        )
        .await?;
        stop_adapter(&mut f, id).await;
        let mut frames = f.editor.debugger_stack_frames()?;
        assert_eq!(frames.frames.len(), 32);
        assert!(frames.has_more);
        let request = command(&mut wire, "stackTrace").await?;
        assert_eq!(request["arguments"]["startFrame"], 0);
        assert_eq!(request["arguments"]["levels"], 32);
        let selected = f
            .editor
            .debugger()
            .get_session(id)
            .unwrap()
            .current_stack_frame()
            .unwrap()
            .id;
        let page = f
            .editor
            .debugger_more_stack_frames(&frames)?
            .run()
            .await
            .unwrap()
            .into_result(&f.editor)
            .unwrap()?;
        frames = page.apply(&mut f.editor).unwrap();
        assert_eq!(frames.frames.len(), 64);
        assert_eq!(
            command(&mut wire, "stackTrace").await?["arguments"]["startFrame"],
            32
        );
        let next = f
            .editor
            .debugger_more_stack_frames(&frames)?
            .run()
            .await
            .unwrap()
            .into_result(&f.editor)
            .unwrap()?;
        let duplicate = f
            .editor
            .debugger_more_stack_frames(&frames)?
            .run()
            .await
            .unwrap()
            .into_result(&f.editor)
            .unwrap()?;
        frames = next.apply(&mut f.editor).unwrap();
        assert!(duplicate.apply(&mut f.editor).is_none());
        assert_eq!(frames.frames.len(), 70);
        assert!(!frames.has_more);
        assert_eq!(
            f.editor
                .debugger()
                .get_session(id)
                .unwrap()
                .current_stack_frame()
                .unwrap()
                .id,
            selected
        );
        f.editor.select_debugger_frame(&frames.context, 69)?;
        assert_eq!(
            f.editor
                .debugger()
                .get_session(id)
                .unwrap()
                .current_stack_frame()
                .unwrap()
                .id,
            169
        );
        f.editor
            .handle_debugger_message(
                id,
                debugger_event("invalidated", json!({"areas":["stacks"]})),
            )
            .await;
        tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
            .await?
            .unwrap()(&mut f.editor);
        assert_eq!(
            f.editor
                .debugger()
                .get_session(id)
                .unwrap()
                .current_stack_frame()
                .unwrap()
                .id,
            169
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn continued_execution_rejects_queued_stack_pages() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (id, _) = adapter_with_options(
        &mut f,
        AdapterOptions {
            stack_count: Some(70),
            ..Default::default()
        },
    )
    .await?;
    stop_adapter(&mut f, id).await;
    let frames = f.editor.debugger_stack_frames()?;
    let page = f
        .editor
        .debugger_more_stack_frames(&frames)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    f.editor
        .handle_debugger_message(id, debugger_event("continued", json!({"threadId":1})))
        .await;
    assert!(page.apply(&mut f.editor).is_none());
    Ok(())
}

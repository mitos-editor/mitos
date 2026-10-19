use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn variable_ranges_are_bounded_shared_and_refresh_restores_later_children(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (id, mut wire) = adapter_with_options(
        &mut f,
        AdapterOptions {
            variable_count: Some(250),
            ..Default::default()
        },
    )
    .await?;
    stop_adapter(&mut f, id).await;
    let scopes = f
        .editor
        .debugger_variables()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    let first = f
        .editor
        .debugger_variable_children(&scopes, 0)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(first.entries().len(), 100);
    assert!(first.has_more());
    let request = command(&mut wire, "variables").await?;
    assert_eq!(request["arguments"]["start"], 0);
    assert_eq!(request["arguments"]["count"], 100);
    let next = f.editor.debugger_more_variables(&first)?;
    let duplicate = f.editor.debugger_more_variables(&first)?;
    let (next, duplicate) = tokio::join!(next.run(), duplicate.run());
    let second = next.unwrap().into_result(&f.editor).unwrap()?;
    assert_eq!(second.entries().len(), 200);
    assert_eq!(
        duplicate
            .unwrap()
            .into_result(&f.editor)
            .unwrap()?
            .entries()
            .len(),
        200
    );
    assert_eq!(
        command(&mut wire, "variables").await?["arguments"]["start"],
        100
    );
    let child = f
        .editor
        .debugger_variable_children(&second, 120)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    f.editor
        .handle_debugger_message(
            id,
            debugger_event("invalidated", json!({"areas":["variables"]})),
        )
        .await;
    let restored = f
        .editor
        .debugger_refresh_variables(&child)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(restored.len(), 3);
    assert_eq!(restored[1].entries()[120].name, "item120");
    let last = f
        .editor
        .debugger_more_variables(&restored[1])?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(last.entries().len(), 250);
    assert!(!last.has_more());
    assert!(f.editor.debugger_more_variables(&last).is_err());
    let queued = f.editor.debugger_more_variables(&first);
    assert!(queued.is_err());
    let queued = f
        .editor
        .debugger_variable_children(&restored[0], 0)?
        .run()
        .await
        .unwrap();
    f.editor
        .handle_debugger_message(id, debugger_event("continued", json!({"threadId":1})))
        .await;
    assert!(queued.into_result(&f.editor).is_none());
    Ok(())
}

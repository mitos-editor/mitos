use super::*;

pub(super) async fn barrier(f: &mut Fixture, wire: &mut AdapterWire) -> anyhow::Result<Vec<Value>> {
    f.editor
        .debugger_threads()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    let mut messages = Vec::new();
    loop {
        let message = message(wire).await?;
        if message["command"] == "threads" {
            return Ok(messages);
        }
        messages.push(message);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn scopes_and_nested_values_load_on_demand_and_concurrent_expansions_share_data(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (_, mut wire) = paused_adapter(&mut f).await?;
    let scopes = f
        .editor
        .debugger_variables()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(scopes.entries().len(), 2);
    assert!(scopes.entries()[1].expensive);
    command(&mut wire, "scopes").await?;
    assert!(barrier(&mut f, &mut wire).await?.is_empty());

    let first = f.editor.debugger_variable_children(&scopes, 0)?;
    let duplicate = f.editor.debugger_variable_children(&scopes, 0)?;
    let expensive = f.editor.debugger_variable_children(&scopes, 1)?;
    let (first, duplicate, expensive) = tokio::join!(first.run(), duplicate.run(), expensive.run());
    let locals = first.unwrap().into_result(&f.editor).unwrap()?;
    assert_eq!(
        duplicate
            .unwrap()
            .into_result(&f.editor)
            .unwrap()?
            .entries()[0]
            .value
            .as_deref(),
        Some("42")
    );
    assert_eq!(
        expensive
            .unwrap()
            .into_result(&f.editor)
            .unwrap()?
            .entries()[0]
            .name,
        "global"
    );
    let mut references: Vec<_> = barrier(&mut f, &mut wire)
        .await?
        .iter()
        .map(|request| {
            assert_eq!(request["command"], "variables");
            request["arguments"]["variablesReference"].as_u64().unwrap()
        })
        .collect();
    references.sort();
    assert_eq!(references, [7, 8]);
    assert!(!locals.entries()[0].is_expandable());
    assert!(f.editor.debugger_variable_children(&locals, 0).is_err());
    assert!(f.editor.debugger_variable_children(&locals, 99).is_err());

    let nested = f
        .editor
        .debugger_variable_children(&locals, 1)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(nested.entries()[0].value.as_deref(), Some("nested value"));
    assert_eq!(
        command(&mut wire, "variables").await?["arguments"]["variablesReference"],
        9
    );
    f.editor
        .debugger_variable_children(&locals, 1)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert!(barrier(&mut f, &mut wire).await?.is_empty());

    let refreshed = f
        .editor
        .debugger_variables()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert!(!scopes.is_current(&f.editor));
    assert!(!locals.is_current(&f.editor));
    f.editor
        .debugger_variable_children(&refreshed, 0)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(command(&mut wire, "scopes").await?["command"], "scopes");
    assert_eq!(
        command(&mut wire, "variables").await?["arguments"]["variablesReference"],
        7
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn resuming_cancels_expansion_and_old_pages_cannot_reuse_adapter_references(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (id, mut wire) = adapter_with_options(
        &mut f,
        AdapterOptions {
            held_commands: vec!["variables"],
            ..Default::default()
        },
    )
    .await?;
    stop_adapter(&mut f, id).await;
    let page = f
        .editor
        .debugger_variables()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    let task = f.editor.debugger_variable_children(&page, 0)?;
    let pending = tokio::spawn(task.run());
    command(&mut wire, "variables").await?;
    f.editor
        .handle_debugger_message(id, debugger_event("continued", json!({"threadId":1})))
        .await;
    assert!(tokio::time::timeout(Duration::from_secs(5), pending)
        .await??
        .is_none());
    stop_adapter(&mut f, id).await;
    assert!(!page.is_current(&f.editor));
    assert!(f.editor.debugger_variable_children(&page, 0).is_err());
    let fresh = f
        .editor
        .debugger_variables()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    let values = f
        .editor
        .debugger_variable_children(&fresh, 0)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(values.entries()[0].value.as_deref(), Some("42"));
    command(&mut wire, "scopes").await?;
    command(&mut wire, "variables").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn abandoned_expansions_can_retry_and_session_selection_revokes_old_pages(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (first, mut wire) = adapter_with_options(
        &mut f,
        AdapterOptions {
            held_commands: vec!["variables"],
            ..Default::default()
        },
    )
    .await?;
    stop_adapter(&mut f, first).await;
    let page = f
        .editor
        .debugger_variables()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    let waiting = tokio::spawn(f.editor.debugger_variable_children(&page, 0)?.run());
    command(&mut wire, "variables").await?;
    waiting.abort();
    assert!(waiting.await.err().unwrap().is_cancelled());
    let values = f
        .editor
        .debugger_variable_children(&page, 0)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(values.entries()[0].value.as_deref(), Some("42"));
    command(&mut wire, "variables").await?;
    let (second, _) = paused_adapter(&mut f).await?;
    f.editor.select_debugger_session(second)?;
    assert!(!page.is_current(&f.editor));
    f.editor.select_debugger_session(first)?;
    assert!(!page.is_current(&f.editor));
    assert!(f.editor.debugger_variable_children(&page, 0).is_err());
    Ok(())
}

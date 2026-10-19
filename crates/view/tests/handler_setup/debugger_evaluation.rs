use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn expandable_evaluations_use_lazy_paged_values_and_expire_without_reexecuting_the_expression(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (id, mut wire) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            variable_count: Some(250),
            ..Default::default()
        },
    )
    .await?;
    let evaluation = f
        .editor
        .debugger_evaluate("object".into())?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    command(&mut wire, "evaluate").await?;
    assert_eq!(evaluation.result, "Object");
    assert!(evaluation.is_expandable());
    assert!(evaluation.is_current(&f.editor));
    let root = evaluation.into_variables(&f.editor).unwrap();
    assert_eq!(root.entries()[0].name, "object");
    f.editor
        .debugger_threads()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(message(&mut wire).await?["command"], "threads"); // Evaluation fetches neither scopes nor children eagerly.
    let first = f
        .editor
        .debugger_variable_children(&root, 0)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(first.entries().len(), 100);
    assert_eq!(
        command(&mut wire, "variables").await?["arguments"]["start"],
        0
    );
    let second = f
        .editor
        .debugger_more_variables(&first)?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(second.entries().len(), 200);
    assert_eq!(
        command(&mut wire, "variables").await?["arguments"]["start"],
        100
    );
    f.editor
        .handle_debugger_message(
            id,
            debugger_event("invalidated", json!({"areas":["variables"]})),
        )
        .await;
    assert!(!root.is_current(&f.editor));
    assert!(!second.can_refresh(&f.editor));
    assert!(f.editor.debugger_refresh_variables(&second).is_err());
    assert!(f.editor.debugger_variable_children(&root, 0).is_err());
    f.editor
        .debugger_threads()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(message(&mut wire).await?["command"], "threads"); // Refreshing must not execute an expression with side effects.
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn evaluation_snapshots_reject_other_editors_queued_invalidation_and_resumed_frames(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (id, _) = paused_adapter(&mut f).await?;
    let evaluation = f
        .editor
        .debugger_evaluate("object".into())?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    let other = Fixture::new("")?;
    assert!(!evaluation.is_current(&other.editor));
    assert!(evaluation.into_variables(&other.editor).is_none());
    let queued = f
        .editor
        .debugger_evaluate("object".into())?
        .run()
        .await
        .unwrap();
    f.editor
        .handle_debugger_message(
            id,
            debugger_event("invalidated", json!({"areas":["variables"]})),
        )
        .await;
    assert!(queued
        .into_result(&f.editor)
        .unwrap()?
        .into_variables(&f.editor)
        .is_none());
    let leaf = f
        .editor
        .debugger_evaluate("42".into())?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(leaf.result, "42");
    assert!(!leaf.is_expandable());
    let root = f
        .editor
        .debugger_evaluate("object".into())?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?
        .into_variables(&f.editor)
        .unwrap();
    f.editor
        .handle_debugger_message(id, debugger_event("continued", json!({"threadId":1})))
        .await;
    assert!(!root.can_refresh(&f.editor));
    assert!(f.editor.debugger_variable_children(&root, 0).is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_evaluations_preserve_the_open_scope_browser() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            failed_commands: vec!["evaluate"],
            ..Default::default()
        },
    )
    .await?;
    let root = f
        .editor
        .debugger_variables()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert!(f
        .editor
        .debugger_evaluate("object".into())?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()
        .is_err());
    assert!(root.is_current(&f.editor));
    Ok(())
}

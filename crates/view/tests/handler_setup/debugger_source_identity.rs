use super::*;
use view::current_ref;

#[tokio::test(flavor = "multi_thread")]
async fn file_source_handles_follow_document_identity_and_editor_ownership() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let document = current_ref!(f.editor).1.id();
    let source = f.editor.debugger_source(document).unwrap();
    assert!(source.is_current(&f.editor));
    assert_eq!(source.local_path(), current_ref!(f.editor).1.path());
    let other = Fixture::new("")?;
    assert!(!source.is_current(&other.editor));
    let renamed = source.local_path().unwrap().with_file_name("renamed.rs");
    f.editor.set_doc_path(document, &renamed);
    assert!(!source.is_current(&f.editor));
    let renamed_source = f.editor.debugger_source(document).unwrap();
    assert!(renamed_source.is_current(&f.editor));
    assert_eq!(renamed_source.local_path(), Some(renamed.as_path()));
    f.close_current()?;
    assert!(!renamed_source.is_current(&f.editor));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn adapter_changes_revoke_source_handles_without_destroying_historical_documents(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let (id, _) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            stack_reference: Some(7),
            source_content: "snapshot".into(),
            ..Default::default()
        },
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !current_ref!(f.editor).1.is_readonly() {
            f.callbacks.recv().await.unwrap()(&mut f.editor);
        }
    })
    .await?;
    let document = current_ref!(f.editor).1.id();
    let source = f.editor.debugger_source(document).unwrap();
    assert!(source.is_current(&f.editor));
    assert!(source.local_path().is_none());
    assert!(!source.is_current(&Fixture::new("")?.editor));
    f.editor
        .handle_debugger_message(
            id,
            debugger_event(
                "loadedSource",
                json!({"reason":"changed","source":{"sourceReference":7}}),
            ),
        )
        .await;
    assert!(!source.is_current(&f.editor));
    assert!(f.editor.debugger_source(document).is_none());
    assert_eq!(f.editor.document(document).unwrap().text(), "snapshot");
    let frames = f.editor.debugger_stack_frames()?;
    f.editor.select_debugger_frame(&frames.context, 0)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while current_ref!(f.editor).1.id() == document {
            f.callbacks.recv().await.unwrap()(&mut f.editor);
        }
    })
    .await?;
    let refreshed = f
        .editor
        .debugger_source(current_ref!(f.editor).1.id())
        .unwrap();
    assert!(refreshed.is_current(&f.editor));
    assert!(!source.is_current(&f.editor));
    f.editor
        .handle_debugger_event(id, dap::ClientEvent::Closed { error: None })
        .await;
    assert!(!refreshed.is_current(&f.editor));
    assert_eq!(f.editor.document(document).unwrap().text(), "snapshot");
    Ok(())
}

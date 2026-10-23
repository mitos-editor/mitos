use super::*;
use view::current_ref;

#[tokio::test(flavor = "multi_thread")]
async fn adapter_file_paths_use_the_same_identity_as_opened_documents() -> anyhow::Result<()> {
    let mut f = Fixture::new("answer")?;
    let document = current_ref!(f.editor).1.id();
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let parent = path.parent().unwrap();
    std::fs::create_dir(parent.join("alias"))?;
    let adapter_path = parent
        .join("alias")
        .join("..")
        .join(path.file_name().unwrap());
    let (_, _) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            stack_source: Some(adapter_path),
            ..Default::default()
        },
    )
    .await?;
    let doc = f.editor.document(document).unwrap();
    assert!(f.editor.debugger_frame_in_document(doc).is_some());
    assert_eq!(f.editor.debugger_execution_line(doc), Some(0));
    Ok(())
}

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
    assert_eq!(
        f.editor
            .debugger_execution_line(f.editor.document(document).unwrap()),
        Some(0)
    );
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
    assert!(f
        .editor
        .debugger_execution_line(f.editor.document(document).unwrap())
        .is_none());
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

#[tokio::test(flavor = "multi_thread")]
async fn execution_lines_reject_unknown_and_out_of_bounds_adapter_locations() -> anyhow::Result<()>
{
    for line in [0, 2, usize::MAX] {
        let mut f = Fixture::new("answer")?;
        let document = current_ref!(f.editor).1.id();
        let path = current_ref!(f.editor).1.path().unwrap().to_owned();
        paused_adapter_with_options(
            &mut f,
            AdapterOptions {
                stack_source: Some(path),
                stack_line: Some(line),
                ..Default::default()
            },
        )
        .await?;
        assert!(f
            .editor
            .debugger_execution_line(f.editor.document(document).unwrap())
            .is_none());
    }
    Ok(())
}

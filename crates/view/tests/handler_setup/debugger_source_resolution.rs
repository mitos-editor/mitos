use super::*;
use editor_core::Rope;
use view::current_ref;

async fn source_document(f: &mut Fixture) -> anyhow::Result<view::DocumentId> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let doc = current_ref!(f.editor).1;
            if doc.is_readonly() {
                return doc.id();
            }
            f.callbacks.recv().await.unwrap()(&mut f.editor);
        }
    })
    .await
    .map_err(Into::into)
}

#[tokio::test(flavor = "multi_thread")]
async fn adapter_sources_override_disk_paths_and_remain_immutable_named_snapshots(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("disk text")?;
    let path = current_ref!(f.editor).1.path().unwrap().to_owned();
    let (_id, mut wire) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            stack_source: Some(path.clone()),
            stack_reference: Some(7),
            source_content: "adapter 😀 text\n".into(),
            ..Default::default()
        },
    )
    .await?;
    let document = source_document(&mut f).await?;
    let request = command(&mut wire, "source").await?;
    assert_eq!(request["arguments"]["sourceReference"], 7);
    let doc = f.editor.document_mut(document).unwrap();
    assert_eq!(doc.text().to_string(), "adapter 😀 text\n");
    assert!(doc.path().is_none());
    assert!(doc.display_name().contains("adapter.rs"));
    assert!(!doc.is_modified());
    doc.detect_readonly();
    assert!(doc.is_readonly());
    let changes = text_diff::compare_ropes(doc.text(), &Rope::from_str("changed"));
    assert_eq!(
        doc.apply(changes.changes()),
        Err(view::editing::EditError::ReadOnlyDocument)
    );
    assert!(doc.save(Some(path.clone()), true).is_err());
    assert_eq!(std::fs::read_to_string(&path)?, "disk text");
    f.editor.set_doc_path(document, &path);
    assert!(f.editor.document(document).unwrap().path().is_none());
    assert!(f
        .editor
        .debugger_frame_in_document(f.editor.document(document).unwrap())
        .is_some());
    let frames = f.editor.debugger_stack_frames()?;
    f.editor.select_debugger_frame(&frames.context, 1)?;
    let frames = f.editor.debugger_stack_frames()?;
    f.editor.select_debugger_frame(&frames.context, 0)?;
    assert_eq!(current_ref!(f.editor).1.id(), document);
    f.editor.new_file(view::editor::Action::Replace);
    assert!(f.editor.close_document(document, true).is_ok());
    let frames = f.editor.debugger_stack_frames()?;
    f.editor.select_debugger_frame(&frames.context, 0)?;
    let reopened = source_document(&mut f).await?;
    assert_ne!(document, reopened);
    assert_eq!(
        current_ref!(f.editor).1.text().to_string(),
        "adapter 😀 text\n"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn source_references_are_scoped_to_the_adapter_and_changes_drop_its_cache(
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
    let first_document = source_document(&mut f).await?;
    let (second, _) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            stack_reference: Some(7),
            source_content: "second".into(),
            ..Default::default()
        },
    )
    .await?;
    // Selecting a stopped session navigates through the same source resolver.
    f.editor.select_debugger_session(second)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while current_ref!(f.editor).1.text() != "second" {
            f.callbacks.recv().await.unwrap()(&mut f.editor);
        }
    })
    .await?;
    let second_document = current_ref!(f.editor).1.id();
    assert_ne!(first_document, second_document);
    assert_eq!(
        f.editor
            .document(first_document)
            .unwrap()
            .text()
            .to_string(),
        "first"
    );
    f.editor
        .handle_debugger_message(
            second,
            debugger_event(
                "loadedSource",
                json!({"reason":"changed","source":{"sourceReference":7}}),
            ),
        )
        .await;
    let frames = f.editor.debugger_stack_frames()?;
    f.editor.select_debugger_frame(&frames.context, 0)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while current_ref!(f.editor).1.id() == second_document {
            f.callbacks.recv().await.unwrap()(&mut f.editor);
        }
    })
    .await?;
    assert_eq!(
        f.editor
            .document(second_document)
            .unwrap()
            .text()
            .to_string(),
        "second"
    );
    assert!(f.editor.debugger().get_session(first).is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_source_replies_cannot_override_intervening_navigation_or_session_removal(
) -> anyhow::Result<()> {
    for fail in [false, true] {
        for remove in [false, true] {
            let mut f = Fixture::new("")?;
            let (id, mut wire) = paused_adapter_with_options(
                &mut f,
                AdapterOptions {
                    failed_commands: if fail { vec!["source"] } else { vec![] },
                    stack_reference: Some(7),
                    source_content: "late".into(),
                    ..Default::default()
                },
            )
            .await?;
            command(&mut wire, "source").await?;
            let callback = tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
                .await?
                .unwrap();
            let destination = if remove {
                f.editor
                    .handle_debugger_event(id, dap::ClientEvent::Closed { error: None })
                    .await;
                current_ref!(f.editor).1.id()
            } else {
                f.editor.new_file(view::editor::Action::Replace)
            };
            f.editor.set_status("newer editor status");
            callback(&mut f.editor);
            assert_eq!(current_ref!(f.editor).1.id(), destination);
            assert_eq!(f.editor.get_status().unwrap().0, "newer editor status");
            assert_ne!(current_ref!(f.editor).1.text().to_string(), "late");
        }
    }
    Ok(())
}

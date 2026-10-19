use super::*;
use editor_core::Transaction;

async fn configured(
    f: &mut Fixture,
) -> anyhow::Result<(DebugAdapterId, AdapterWire, std::path::PathBuf)> {
    let path = view::current_ref!(f.editor).1.path().unwrap().to_owned();
    f.editor.toggle_breakpoints(path.clone(), &[1])?;
    let (id, mut wire) = adapter(f).await?;
    f.editor
        .handle_debugger_message(id, debugger_event("initialized", json!({})))
        .await;
    let initial = command(&mut wire, "setBreakpoints")
        .await
        .context("initial breakpoint configuration")?;
    assert_eq!(initial["arguments"]["sourceModified"], false);
    debugger_callback(f)
        .await
        .context("initial breakpoint callback")?;
    command(&mut wire, "configurationDone")
        .await
        .context("configuration completion")?;
    debugger_callback(f).await?;
    Ok((id, wire, path))
}

async fn verified(f: &mut Fixture, identity: view::debugger::BreakpointId) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f
            .editor
            .debugger()
            .breakpoint_binding(identity)
            .is_some_and(|binding| binding.verified)
        {
            debugger_callback(f).await?;
        }
        anyhow::Ok(())
    })
    .await?
}

#[tokio::test(flavor = "multi_thread")]
async fn unsaved_markers_are_local_until_successful_save() -> anyhow::Result<()> {
    let mut f = Fixture::new("first\nmarked\n")?;
    let (_, mut wire, path) = configured(&mut f).await?;
    let identity = f.editor.debugger().breakpoints(&path)[0].identity();
    let doc = view::current!(f.editor).1;
    let id = doc.id();
    let edit = Transaction::change(doc.text(), [(0, 0, Some("added\n".into()))].into_iter());
    doc.apply(edit.changes())?;
    assert_eq!(
        f.editor
            .debugger()
            .breakpoint_locations(&path)
            .next()
            .unwrap()
            .line,
        2
    );
    assert!(
        !f.editor
            .debugger()
            .breakpoint_locations(&path)
            .next()
            .unwrap()
            .verified
    );
    // A transport barrier proves no automatic breakpoint request was sent for unsaved text.
    f.editor
        .debugger_threads()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(message(&mut wire).await?["command"], "threads");
    f.editor.save(id, None::<std::path::PathBuf>, false)?;
    f.editor.flush_writes().await?;
    let update = command(&mut wire, "setBreakpoints")
        .await
        .context("saved/reloaded source update")?;
    assert_eq!(update["arguments"]["breakpoints"][0]["line"], 3);
    assert_eq!(update["arguments"]["sourceModified"], true);
    verified(&mut f, identity).await?;
    assert!(
        f.editor
            .debugger()
            .breakpoint_locations(&path)
            .next()
            .unwrap()
            .verified
    );
    let doc = f.editor.document_mut(id).unwrap();
    let edit = Transaction::change(doc.text(), [(0, 0, Some("more\n".into()))].into_iter());
    doc.apply(edit.changes())?;
    f.editor.toggle_breakpoints(path.clone(), &[3])?;
    assert!(f.editor.debugger().breakpoints(&path).is_empty());
    f.editor.save(id, None::<std::path::PathBuf>, false)?;
    f.editor.flush_writes().await?;
    let clear = command(&mut wire, "setBreakpoints").await?;
    assert_eq!(clear["arguments"]["breakpoints"], json!([]));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn saving_and_renaming_sources_without_breakpoints_does_not_replace_adapter_breakpoints(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("ordinary source\n")?;
    let (id, mut wire) = adapter(&mut f).await?;
    f.editor
        .handle_debugger_message(id, debugger_event("initialized", json!({})))
        .await;
    command(&mut wire, "configurationDone").await?;
    debugger_callback(&mut f).await?;
    let path = view::current_ref!(f.editor).1.path().unwrap().to_owned();
    let doc = view::current!(f.editor).1;
    let id = doc.id();
    let edit = Transaction::change(doc.text(), [(0, 0, Some("added\n".into()))].into_iter());
    doc.apply(edit.changes())?;
    f.editor.save(id, None::<std::path::PathBuf>, false)?;
    f.editor.flush_writes().await?;
    f.editor
        .debugger_threads()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(message(&mut wire).await?["command"], "threads");
    f.editor
        .request_file_operation(view::editor::FileOperation::Move {
            from: path.clone(),
            to: path.with_file_name("renamed.words"),
        })?;
    f.editor
        .debugger_threads()?
        .run()
        .await
        .unwrap()
        .into_result(&f.editor)
        .unwrap()?;
    assert_eq!(message(&mut wire).await?["command"], "threads");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn reload_and_rename_update_sources_and_clear_the_old_path() -> anyhow::Result<()> {
    let mut f = Fixture::new("first\nmarked\n")?;
    let (_, mut wire, path) = configured(&mut f).await?;
    let identity = f.editor.debugger().breakpoints(&path)[0].identity();
    let id = view::current_ref!(f.editor).1.id();
    std::fs::write(&path, "added\nfirst\nmarked\n")?;
    f.editor.reload_document(id)?;
    let update = command(&mut wire, "setBreakpoints")
        .await
        .context("saved/reloaded source update")?;
    assert_eq!(update["arguments"]["breakpoints"][0]["line"], 3);
    assert_eq!(update["arguments"]["sourceModified"], true);
    verified(&mut f, identity).await?;
    let renamed = path.with_file_name("renamed.words");
    f.editor
        .request_file_operation(view::editor::FileOperation::Move {
            from: path.clone(),
            to: renamed.clone(),
        })?;
    let updates = [
        command(&mut wire, "setBreakpoints").await?,
        command(&mut wire, "setBreakpoints").await?,
    ];
    let old = updates
        .iter()
        .find(|update| update["arguments"]["source"]["path"] == json!(path))
        .unwrap();
    let new = updates
        .iter()
        .find(|update| update["arguments"]["source"]["path"] == json!(renamed))
        .unwrap();
    assert_eq!(old["arguments"]["breakpoints"], json!([]));
    assert_eq!(new["arguments"]["breakpoints"][0]["line"], 3);
    assert!(f.editor.debugger().breakpoints(&path).is_empty());
    assert_eq!(
        f.editor.debugger().breakpoints(&renamed)[0].identity(),
        identity
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_dirty_document_restores_saved_positions_and_accepted_writes_can_advance_them(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("first\nmarked\n")?;
    let path = view::current_ref!(f.editor).1.path().unwrap().to_owned();
    f.editor.toggle_breakpoints(path.clone(), &[1])?;
    let doc = view::current!(f.editor).1;
    let id = doc.id();
    let edit = Transaction::change(doc.text(), [(0, 0, Some("added\n".into()))].into_iter());
    doc.apply(edit.changes())?;
    f.editor.save(id, None::<std::path::PathBuf>, false)?;
    f.editor.new_file(view::editor::Action::Replace);
    assert!(f.editor.close_document(id, true).is_ok());
    assert_eq!(f.editor.debugger().breakpoints(&path)[0].line, 1);
    f.editor.flush_writes().await?;
    assert_eq!(f.editor.debugger().breakpoints(&path)[0].line, 2);
    Ok(())
}
